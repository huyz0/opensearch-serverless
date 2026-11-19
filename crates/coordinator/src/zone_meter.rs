//! Counting the bytes a coordinator exchanges with peers, by zone.
//!
//! Each call to a peer is one exchange, counted once by the node that
//! started it: the request bytes when the request goes out, the answer
//! bytes when an answer comes back. The peer's zone is read from the lease
//! it advertises, so classifying adds no request.

use oss_cluster::zones::{Path, ZoneTraffic};
use oss_format::NodeLease;

/// A node's view for counting: its own zone, the leases it knows and the
/// shared meter.
#[derive(Clone, Copy, Debug)]
pub struct ZoneMeter<'a> {
    traffic: &'a ZoneTraffic,
    local: Option<&'a str>,
    members: &'a [NodeLease],
}

impl<'a> ZoneMeter<'a> {
    /// A meter view for a node in `local` that knows `members`.
    pub fn new(traffic: &'a ZoneTraffic, local: Option<&'a str>, members: &'a [NodeLease]) -> Self {
        Self {
            traffic,
            local,
            members,
        }
    }

    /// Runs `call` once, the exchange with `peer_id` for `action`.
    ///
    /// Counts `request_bytes` before the call, since a request that went out
    /// is billed even when the call fails, and `answer_bytes` of the answer
    /// when it succeeds. The call's result is returned unchanged.
    ///
    /// # Errors
    ///
    /// Whatever `call` returns.
    pub fn exchange<T, E>(
        &self,
        peer_id: &str,
        action: &str,
        request_bytes: u64,
        call: impl FnOnce() -> Result<T, E>,
        answer_bytes: impl FnOnce(&T) -> u64,
    ) -> Result<T, E> {
        let path = Path::to_peer(self.local, self.members, peer_id);
        self.traffic.record(path, action, request_bytes);
        let answer = call()?;
        self.traffic.record(path, action, answer_bytes(&answer));
        Ok(answer)
    }
}
