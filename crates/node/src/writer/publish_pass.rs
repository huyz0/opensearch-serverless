//! The publish pass: which held shards are due, and publishing them, at
//! once when packs are on so that their blobs share a pack.

use std::sync::{Arc, Mutex};

use super::bounded::run_bounded;
use super::publishing::Key;
use super::slot::ShardSlot;
use super::{PublishPass, Publishing, WriterRole, lock};

/// What the pass did for one shard.
type Done = Option<Result<(), String>>;

impl WriterRole {
    /// Publishes the held shards the lazy policy finds due, at most
    /// `pass_threads` at once, and counts them.
    pub(super) fn publish_held(&self, publishing: &Publishing) -> PublishPass {
        // ALLOC: one handle a held shard, once a pass.
        let held: Vec<(Key, Arc<Mutex<ShardSlot>>)> = lock(&self.shards)
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        let threads = publishing.pass_threads();
        // With one thread each shard decides for itself, as the pass always
        // did; with more, only due shards are handed to threads.
        let work: Vec<_> = if threads > 1 {
            held.into_iter()
                .filter(|(key, slot)| self.due_now(publishing, key, slot))
                .collect()
        } else {
            held
        };
        let mut pass = PublishPass::default();
        for done in run_bounded(&work, threads, |(key, slot)| {
            self.publish_one(publishing, key, slot)
        }) {
            match done {
                None => {}
                Some(Ok(())) => pass.published = pass.published.saturating_add(1),
                Some(Err(reason)) => pass.failed.push(reason),
            }
        }
        pass
    }

    /// Whether `key` could be published now, by the checks of
    /// [`Self::publish_shard`]'s first block.
    fn due_now(&self, publishing: &Publishing, key: &Key, slot: &Arc<Mutex<ShardSlot>>) -> bool {
        let held = lock(slot);
        !(held.failed() || held.released() || !publishing.due(key, held.acked_seq_no()))
    }

    fn publish_one(
        &self,
        publishing: &Publishing,
        key: &Key,
        slot: &Arc<Mutex<ShardSlot>>,
    ) -> Done {
        let index = self.names.name_of(&key.0)?;
        match self.publish_shard(publishing, key, &index, slot, true) {
            Ok(None) => None,
            Ok(Some((done, max_seq_no))) => {
                // Staged before the tail drops what the commit holds, so a
                // reader opening in between finds one or the other.
                self.staged(&key.0, key.1, &done.manifest);
                // A staged commit is not in the register: the tail keeps its
                // records until a register write holds them.
                if !done.staged
                    && let Some(t) = self.tail.get()
                {
                    t.published(&key.0, key.1, max_seq_no);
                }
                Some(Ok(()))
            }
            // ALLOC: error path; one message a failed shard.
            Err(reason) => Some(Err(format!("[{}][{}] {reason}", key.0, key.1))),
        }
    }
}
