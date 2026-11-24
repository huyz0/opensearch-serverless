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
        // The deadline covers the server's shutdown too, which waits for
        // in-flight requests.
        let now = crate::writer::now_millis();
        let role = self.running.take().map(|running| {
            let role = std::sync::Arc::clone(&running.role);
            running.server.shutdown();
            role
        });
        drop(self.transport.take());
        match role {
            Some(role) if timeout_millis > 0 => {
                role.retire(now, now.saturating_add(timeout_millis))
            }
            _ => HibernatePass::default(),
        }
    }
}

/// How the retire of a node ended when bounded by a wall-clock limit.
#[derive(Debug)]
pub enum Retired {
    /// It finished, and says what it did.
    Done(HibernatePass),
    /// A blocking call (a store request, a request being served) outlasted
    /// the limit; the node is left to the lease and the process should end.
    Overran,
}

/// Runs `work` on its own thread and waits at most `limit` for it, so a
/// call that never returns cannot hold the process past its stop.
pub fn within<T: Send + 'static>(
    limit: std::time::Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = done.send(work());
    });
    wait.recv_timeout(limit).ok()
}

/// [`Node::retire`] bounded by `timeout_millis` plus a grace for what no
/// deadline check can interrupt (one blocked store call, a request being
/// served).
pub fn retire_within(node: Node, timeout_millis: u64) -> Retired {
    let limit = std::time::Duration::from_millis(timeout_millis.saturating_add(GRACE_MILLIS));
    within(limit, move || node.retire(timeout_millis)).map_or(Retired::Overran, Retired::Done)
}

/// What a retire may overrun its deadline by before the process gives up.
pub const GRACE_MILLIS: u64 = 5_000;
