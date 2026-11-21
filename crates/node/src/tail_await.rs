//! Waiting for the nodes a shard is pushed to before a refreshing write is
//! answered.
//!
//! Ported from `awaitSearchNodes` of the shell's `TailPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`)
//! and its `RefreshVisibility` modes. Blocking: the caller runs it where a
//! write may wait.

use std::time::Duration;

use crate::tail_push::TailPush;

/// How long a refreshing write waits for the nodes a shard is pushed to, at
/// most: the shell's `RefreshVisibility.WAIT_MILLIS`.
pub const WAIT: Duration = Duration::from_millis(500);

impl TailPush {
    /// Waits until every node the shard is pushed to has taken every record
    /// this node has acknowledged for it so far, or `timeout` passes, pushing
    /// what is queued at once. With `grants` (the `everywhere` visibility
    /// mode) it also waits until every grant this node can no longer reach has
    /// run out. A node that subscribes meanwhile needs no wait: its fetch's
    /// answer holds what was kept when it subscribed.
    ///
    /// Returns whether every node was reached: false when one was lost, the
    /// wait timed out, or the shard was let go meanwhile.
    pub fn await_search_nodes(
        &self,
        uuid: &str,
        shard: u32,
        timeout: Duration,
        grants: bool,
    ) -> bool {
        self.serve.claim(uuid, shard);
        let keep = &self.keep;
        if grants && !keep.keeps(uuid, shard) {
            self.serve.note_kept(uuid, shard);
            keep.begin(uuid, shard, self.facts.max_seq_no(uuid, shard));
        }
        if !keep.keeps(uuid, shard) {
            return true;
        }
        let target = keep.position(uuid, shard);
        let lapses_at = grants.then(|| self.serve.lapse_deadline(uuid, shard));
        let wait = self.serve.open_wait(uuid, shard, target);
        if let Some(id) = wait {
            self.flush();
            if !self.serve.wait(id, timeout) {
                return false;
            }
        }
        if let Some(at) = lapses_at {
            self.serve.pause_until(at);
        }
        true
    }
}
