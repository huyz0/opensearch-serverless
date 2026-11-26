//! A refresh: the engine's writes become visible, and where the role
//! publishes, the commit that holds them is published before it returns, so
//! a reader that asks afterwards finds them.

use oss_writer::bulk::{IndexInfo, Owner};

use std::sync::{Arc, Mutex};

use oss_writer::publish::Published;

use super::OpenRefused;
use super::publishing::Key;
use super::slot::ShardSlot;
use super::{Publishing, WriterRole, lock};

impl WriterRole {
    /// Refreshes a shard on this node when it holds it, and otherwise asks
    /// the node that owns it to.
    ///
    /// # Errors
    ///
    /// As [`WriterRole::refresh`], or the owner not answering.
    pub fn refresh_owned(&self, index: &IndexInfo, shard: u32) -> Result<(), String> {
        match self.owner(index, shard) {
            Owner::Local => self.refresh(index, shard),
            Owner::Node(node) => {
                let done = self.routes.refresh_remote(&node, index, shard);
                if done.is_err() {
                    self.unsteer(&index.uuid, shard);
                }
                done
            }
        }
    }

    /// Refreshes a shard held here and, where the role publishes, publishes
    /// its commit now.
    ///
    /// # Errors
    ///
    /// The shard not held, or the engine's or the publish's.
    pub fn refresh(&self, index: &IndexInfo, shard: u32) -> Result<(), String> {
        let slot = match self.slot(&index.uuid, shard) {
            Some(slot) => slot,
            None if self.asleep(&index.uuid, shard) => {
                // Everything it took is in a published commit.
                return Ok(());
            }
            // Asked as the owner of a shard it has not opened: the writes
            // acknowledged for it are in the node logs, and opening it, as a
            // write would, replays them before the refresh.
            None => match self.slot_or_open(index, shard) {
                Ok(slot) => slot,
                Err(OpenRefused {
                    holder: Some(holder),
                    ..
                }) => {
                    // A live node holds it: the refresh goes where a write
                    // would have, and the next call finds it.
                    // ALLOC: one entry a shard another node holds.
                    lock(&self.redirects).insert((index.uuid.clone(), shard), holder.clone());
                    let done = self.routes.refresh_remote(&holder, index, shard);
                    if done.is_err() {
                        lock(&self.redirects).remove(&(index.uuid.clone(), shard));
                    }
                    return done;
                }
                Err(refused) => return Err(refused.reason),
            },
        };
        lock(&slot).refresh()?;
        let Some(publishing) = &self.publishing else {
            return Ok(());
        };
        let visibility = self.refresh_visibility();
        if visibility.waits() {
            // Outside the shard lock: the wait is for readers, and writes
            // go on meanwhile.
            let confirmed = self
                .tail
                .get()
                .is_some_and(|sink| sink.await_visible(&index.uuid, shard, visibility.grants()));
            if confirmed {
                return Ok(());
            }
        }
        // ALLOC: one key a refresh.
        let key = (index.uuid.clone(), shard);
        let done = self
            .publish_shard(publishing, &key, &index.name, &slot, false)?
            .map(|(done, _)| done);
        if let Some(done) = done {
            self.staged(&index.uuid, shard, &done.manifest);
        }
        Ok(())
    }

    /// Publishes the commit of `slot`, when `only_if_due` only if the lazy
    /// policy says it is due and the shard is not released: the commit is
    /// taken under the shard's lock and uploaded after it is let go, so
    /// writes and refreshes go on meanwhile. Commits are published in the
    /// order they were taken. The manifest and the highest sequence number
    /// it holds; `None` when nothing was published.
    ///
    /// # Errors
    ///
    /// The engine's or the publish's.
    pub(super) fn publish_shard(
        &self,
        publishing: &Publishing,
        key: &Key,
        index: &str,
        slot: &Arc<Mutex<ShardSlot>>,
        only_if_due: bool,
    ) -> Result<Option<(Published, i64)>, String> {
        if only_if_due {
            // Not due, or the slot is going: no quiesce, no gate.
            if !self.due_now(publishing, key, slot) {
                return Ok(None);
            }
        }
        let _gate = publishing.gate(key);
        let (snapshot, term, max_seq_no, logged, shard_key) = {
            // No group of the shard is in flight and none stages until the
            // snapshot is taken, so every write in the commit is in the log.
            let (mut held, _quiet) = super::pipelined::quiet_slot(slot);
            // QUIESCED: no group of the shard is in flight until the snapshot.
            let max_seq_no = held.max_seq_no();
            if held.failed() || held.released() || only_if_due && !publishing.due(key, max_seq_no) {
                return Ok(None);
            }
            // What the log holds of this shard now is in the commit the
            // snapshot takes.
            let logged = self
                .committer
                .last_position(held.key())
                .map(|at| (held.key().clone(), at));
            let (snapshot, term) = held.snapshot()?;
            (snapshot, term, max_seq_no, logged, held.key().clone())
        };
        let done = publishing.with_publisher(key, index, |publisher, node, now| {
            let done = if only_if_due {
                publisher.publish_routine(&snapshot, snapshot.names(), term, node, -1, now)
            } else {
                publisher.publish(&snapshot, snapshot.names(), term, node, -1, now)
            }
            .map_err(|e| e.to_string())?;
            Ok((done, publisher.staged_age()))
        })?;
        let (done, age) = done;
        if done.staged {
            // The register does not hold it: the shard still counts as
            // unpublished, and the log is trimmed when a register write
            // lands. The lazy policy's due clock moves on, or the shard
            // would be due at every pass.
            let covered = logged.as_ref().map(|(_, at)| *at);
            publishing.stage_pending(key, logged, max_seq_no);
            self.log_staged(&shard_key, term, age, covered, &done.manifest, max_seq_no);
            publishing.published(key, max_seq_no);
            return Ok(Some((done, max_seq_no)));
        }
        publishing.published(key, max_seq_no);
        self.log_checkpointed(key, max_seq_no);
        if let Some((shard, at)) = publishing.take_pending(key, logged).0 {
            // The log no longer needs these records: its lease stops naming
            // epochs that hold nothing else, and a successor has less to take.
            self.committer.on_published(&shard, at);
        }
        Ok(Some((done, max_seq_no)))
    }

    /// Writes the register for every shard whose staged commit has reached
    /// its checkpoint age, with no snapshot and no upload. A shard whose
    /// staged commit was dropped is published in full instead.
    pub(super) fn checkpoint_staged(&self, publishing: &Publishing) -> Vec<String> {
        let mut failed = Vec::new();
        for key in publishing.staged_keys() {
            let Some(index) = self.names.name_of(&key.0) else {
                // The index is gone: nothing to write. The log stays untrimmed.
                publishing.take_pending(&key, None);
                continue;
            };
            // What was staged is taken under the gate that flushed it: a
            // publish staging a newer commit afterwards keeps its own entry.
            let (written, taken) = {
                let _gate = publishing.gate(&key);
                let written = publishing.with_publisher(&key, &index, |publisher, _, _| {
                    publisher.flush_due().map_err(|e| e.to_string())
                });
                let taken =
                    matches!(written, Ok(Some(_))).then(|| publishing.take_pending(&key, None));
                (written, taken)
            };
            if let Some((logged, through)) = taken {
                self.log_checkpointed(&key, through);
                if let Some((shard, at)) = logged {
                    self.committer.on_published(&shard, at);
                }
                if let Some(t) = self.tail.get() {
                    t.published(&key.0, key.1, through);
                }
                continue;
            }
            if written.is_ok() {
                continue;
            }
            let Some(slot) = self.slot(&key.0, key.1) else {
                publishing.take_pending(&key, None);
                continue;
            };
            match self.publish_shard(publishing, &key, &index, &slot, false) {
                Ok(Some((done, max_seq_no))) => {
                    self.staged(&key.0, key.1, &done.manifest);
                    if let Some(t) = self.tail.get() {
                        t.published(&key.0, key.1, max_seq_no);
                    }
                }
                Ok(None) => {}
                Err(reason) => {
                    // ALLOC: error path; one message a failed shard.
                    failed.push(format!("[{}][{}] {reason}", key.0, key.1));
                }
            }
        }
        failed
    }
}
