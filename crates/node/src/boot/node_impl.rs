//! What a running [`Node`] answers and how it stops.

use super::*;

impl Node {
    /// Stops the node as a killed process stops: its servers and its
    /// renewal end, and its lease is not released, so it lapses in one
    /// lifetime and the other nodes see it die.
    pub fn crash(mut self) {
        self.crashed = true;
    }

    /// The address the node's HTTP server listens on.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.running.as_ref().map_or_else(
            || std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
            Running::addr,
        )
    }

    /// Whether the tail overlay is on: writers push to this node's reader
    /// side and its searches see what they pushed.
    pub fn tail_on(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.tail().is_some())
    }

    /// The refresh visibility the node is in, which follows the cluster
    /// register; `None` while the tail overlay is off.
    pub fn refresh_visibility(&self) -> Option<crate::writer::RefreshVisibility> {
        self.following.as_ref().map(follow::Following::mode)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.edge.stop();
        // The renewal stops first, so it cannot write the lease back.
        drop(self.renewal.take());
        drop(self.dead_logs.take());
        drop(self.orphan_sweep.take());
        drop(self.pack_collect.take());
        drop(self.ad_loop.take());
        drop(self.following.take());
        drop(self.running.take());
        drop(self.transport.take());
        // Best effort: an unreleased lease lapses in one lifetime.
        if !self.crashed {
            let _ = self.membership.release(&self.node_id);
        }
    }
}
