//! A node told to stop: it stops serving, gives its shards back, and
//! releases its lease.

use super::Node;
use crate::writer::HibernatePass;

impl Node {
    /// Stops serving first, so nothing wakes a shard again, then publishes
    /// and gives back every shard the writer role holds. Dropping the node
    /// afterwards releases its member lease. The answer says what the
    /// writer role did; a shard in `failed` keeps its head.
    ///
    /// `timeout_millis` bounds the publishing and giving back; shards not
    /// reached by then are counted `unfinished` and left to the lease. 0
    /// skips them all and only stops the node and releases the lease.
    pub fn retire(mut self, timeout_millis: u64) -> HibernatePass {
        // The renewal and timers stop first; the role is retired after the
        // server that feeds it is gone.
        drop(self.renewal.take());
        drop(self.dead_logs.take());
        drop(self.orphan_sweep.take());
        drop(self.following.take());
        let role = self.running.take().map(|running| {
            let role = std::sync::Arc::clone(&running.role);
            running.server.shutdown();
            role
        });
        drop(self.transport.take());
        let now = crate::writer::now_millis();
        match role {
            Some(role) if timeout_millis > 0 => {
                role.retire(now, now.saturating_add(timeout_millis))
            }
            _ => HibernatePass::default(),
        }
    }
}
