//! Whether a node is live, asked at a takeover, and revoking a dead one's
//! lease once for every takeover of its shards running at that moment.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/LeaseLivenessOracle.java`.
//! A takeover reads the owner's lease itself rather than trusting the
//! refreshed view: taking a shard from a node that is alive is the one
//! mistake the protocol exists to prevent.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use super::lock;
use super::membership::Membership;

/// One revocation in flight, which concurrent revocations of the same
/// process wait for.
#[derive(Default)]
struct Flight {
    answer: Mutex<Option<bool>>,
    done: Condvar,
}

impl Flight {
    fn wait(&self) -> bool {
        let mut answer = lock(&self.answer);
        loop {
            if let Some(answer) = *answer {
                return answer;
            }
            answer = self
                .done
                .wait(answer)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn finish(&self, answer: bool) {
        *lock(&self.answer) = Some(answer);
        self.done.notify_all();
    }
}

/// Answers whether a node is live from its lease, and revokes a dead one's.
pub struct LivenessOracle {
    membership: Arc<Membership>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
}

/// Ends a revocation when dropped, so one that panics answers no and never
/// leaves the others waiting.
struct Lead<'a> {
    oracle: &'a LivenessOracle,
    key: String,
    flight: Arc<Flight>,
    answer: bool,
}

impl Drop for Lead<'_> {
    fn drop(&mut self) {
        lock(&self.oracle.flights).remove(&self.key);
        self.flight.finish(self.answer);
    }
}

impl LivenessOracle {
    /// The oracle over `membership`'s leases.
    pub fn new(membership: Arc<Membership>) -> Self {
        Self {
            membership,
            // ALLOC: one entry a revocation in flight, at most the takeovers running.
            flights: Mutex::new(HashMap::new()),
        }
    }

    /// Whether `node_id` holds an unexpired lease of process `ephemeral_id`
    /// (any process when `None`): one read. An ephemeral id that moved on
    /// means the process that held the claim is gone, even if a node of
    /// that name is back. A lease that cannot be read is live: refusing a
    /// takeover costs a retry, and the alternative costs two writers.
    pub fn is_live(&self, node_id: &str, ephemeral_id: Option<&str>) -> bool {
        match self.membership.read(node_id) {
            Ok(Some(lease)) => {
                !lease.is_expired_at(self.membership.clock.now_millis())
                    && ephemeral_id.is_none_or(|e| e == lease.ephemeral_id)
            }
            Ok(None) => false,
            Err(_) => true,
        }
    }

    /// [`Membership::revoke`], shared among the revocations of one process
    /// running at the same moment: the first reads and swaps, the rest wait
    /// for its answer. Only calls in flight share one, so a node that
    /// renews afresh after a revocation is judged by a read made after.
    pub fn revoke(&self, node_id: &str, ephemeral_id: Option<&str>) -> bool {
        // ALLOC: one key a revocation.
        let key = format!("{node_id}#{}", ephemeral_id.unwrap_or(""));
        let flight = {
            let mut flights = lock(&self.flights);
            if let Some(running) = flights.get(&key) {
                let running = Arc::clone(running);
                drop(flights);
                return running.wait();
            }
            let flight = Arc::new(Flight::default());
            // ALLOC: as above.
            flights.insert(key.clone(), Arc::clone(&flight));
            flight
        };
        let mut lead = Lead {
            oracle: self,
            key,
            flight,
            answer: false,
        };
        lead.answer = self.membership.revoke(node_id, ephemeral_id);
        lead.answer
    }
}
