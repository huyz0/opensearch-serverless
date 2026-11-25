//! A refresh: the engine's writes become visible, and where the role
//! publishes, the commit that holds them is published before it returns, so
//! a reader that asks afterwards finds them.

use oss_writer::bulk::{IndexInfo, Owner};

use std::sync::{Arc, Mutex};

use oss_writer::publish::Published;

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
        let Some(slot) = self.slot(&index.uuid, shard) else {
            if self.asleep(&index.uuid, shard) {
                // Everything it took is in a published commit.
                return Ok(());
            }
            // ALLOC: error path; one message.
            return Err(format!(
                "[{}][{shard}] is not open on this node",
                index.name
            ));
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
            let held = lock(slot);
            if held.failed() || held.released() || !publishing.due(key, held.acked_seq_no()) {
                return Ok(None);
            }
        }
        let _gate = publishing.gate();
        let (snapshot, term, max_seq_no, logged) = {
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
            (snapshot, term, max_seq_no, logged)
        };
        let done = publishing.with_publisher(key, index, |publisher, node, now| {
            publisher
                .publish(&snapshot, snapshot.names(), term, node, -1, now)
                .map_err(|e| e.to_string())
        })?;
        publishing.published(key, max_seq_no);
        if let Some((shard, at)) = logged {
            // The log no longer needs these records: its lease stops naming
            // epochs that hold nothing else, and a successor has less to take.
            self.committer.on_published(&shard, at);
        }
        Ok(Some((done, max_seq_no)))
    }
}
