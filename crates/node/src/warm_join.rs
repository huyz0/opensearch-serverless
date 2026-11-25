//! A joining reader warming its cache from every peer: who is asked, the
//! signed transport calls, and the bounded fan-out over peers.
//!
//! The peers are the other nodes whose lease shows the `search` role and is
//! not revoked, read from the leases the node already holds, so finding them
//! makes no request. At most `parallel_peers` are copied from at once, each
//! holding one batch of pages, which is reserved from the memory budget
//! before the first is asked. A peer that fails is skipped; the rest go on.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lucene_store::page_cache::PageCache;
use oss_cluster::placement::ROLE_SEARCH;
use oss_mem::{MemoryBudget, Pool};
use oss_transport::actions::warm::{
    HOT_SET_ACTION, HotSetRequest, HotSetResponse, PAGES_ACTION, PagesRequest, PagesResponse,
};
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Call, Client, PeerId};
use oss_transport::stream::{Input, Output};

use crate::push_link::Leases;
use crate::warm_copy::{Outcome, Pause, Peer, Progress, Settings, Shards, copy_from_peer};

/// How long a joiner waits for a peer's hot set.
pub const HOT_SET_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a joiner waits for a peer's batch of pages.
pub const PAGES_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a request takes on the wire.
const MAX_ASK_BYTES: usize = 64 << 10;

/// The bytes one peer's batch holds at most: 64 pages of the default size.
pub const BATCH_BYTES: usize = 4 << 20;

/// The ids of the other live search nodes, in the order the leases list them.
pub fn peers_of(leases: &dyn Leases, self_id: &str) -> Vec<String> {
    leases
        .current()
        .iter()
        .filter(|l| {
            !l.revoked && l.node_id != self_id && l.roles().iter().any(|r| r == ROLE_SEARCH)
        })
        .map(|l| l.node_id.clone())
        .collect()
}

/// One peer node, called over the transport client, signed.
pub struct TransportPeer<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
    node: String,
}

impl<L: Leases> TransportPeer<L> {
    /// The node `node`, found in `leases`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, leases: L, node: &str) -> Self {
        Self {
            client,
            auth,
            leases,
            // ALLOC: one id per peer.
            node: node.to_owned(),
        }
    }

    fn call(
        &self,
        action: &'static str,
        timeout: Duration,
        write: impl FnOnce(&mut Output<'_>) -> Result<(), oss_transport::error::WireError>,
    ) -> Option<Vec<u8>> {
        let leases = self.leases.current();
        let lease = leases
            .iter()
            .find(|l| l.node_id == self.node && !l.revoked)?;
        let address: SocketAddr = lease.address.parse().ok()?;
        // ALLOC: one request message, at most MAX_ASK_BYTES.
        let mut message = Vec::new();
        write(&mut Output::new(&mut message, MAX_ASK_BYTES)).ok()?;
        let mac = self.auth.sign(
            action,
            &message,
            &identity(&lease.node_id, &lease.ephemeral_id),
            &[],
        )?;
        // ALLOC: the MAC header, one entry.
        let headers = [(MAC_HEADER.to_owned(), mac)];
        let peer = PeerId {
            address,
            // ALLOC: one ephemeral id a call.
            ephemeral_id: Some(lease.ephemeral_id.clone()),
        };
        let call = Call {
            action,
            headers: &headers,
            compress: false,
            deadline: Instant::now().checked_add(timeout)?,
        };
        let answer = self
            .client
            .call(&peer, call, |out| out.bytes(&message))
            .ok()?;
        Some(answer.message().to_vec())
    }
}

impl<L: Leases> Peer for TransportPeer<L> {
    fn hot_set(&self, request: &HotSetRequest) -> Option<HotSetResponse> {
        let answer = self.call(HOT_SET_ACTION, HOT_SET_TIMEOUT, |out| request.write(out))?;
        HotSetResponse::read(&mut Input::new(&answer)).ok()
    }

    fn pages(&self, request: &PagesRequest) -> Option<PagesResponse> {
        let answer = self.call(PAGES_ACTION, PAGES_TIMEOUT, |out| request.write(out))?;
        PagesResponse::read(&mut Input::new(&answer)).ok()
    }
}

/// What a warm-up is given besides the peers.
pub struct Plan<'a> {
    /// The copy's per-peer settings.
    pub settings: Settings<'a>,
    /// The most peers copied from at once; at least 1.
    pub parallel_peers: usize,
    /// The pool the batches are reserved from.
    pub pool: Pool,
    /// How the copy waits between batches.
    pub pause: &'a (dyn Pause + Sync),
    /// Whether the wait is over.
    pub expired: &'a (dyn Fn() -> bool + Sync),
}

/// Copies from each of `peers` in turn, `parallel_peers` at a time, and
/// returns each peer's outcome. Returns nothing when the batches do not fit
/// the budget: a joiner short of memory serves cold.
pub fn warm_from<'a>(
    peers: &[Box<dyn Peer + Send + Sync + 'a>],
    shards: &(dyn Shards + Sync),
    cache: &PageCache,
    budget: &MemoryBudget,
    plan: &Plan<'_>,
    progress: &Progress,
) -> Vec<Outcome> {
    let workers = plan.parallel_peers.max(1).min(peers.len());
    let Some(bytes) = BATCH_BYTES.checked_mul(workers) else {
        return Vec::new();
    };
    let Ok(_held) = budget.try_reserve(plan.pool, bytes) else {
        return Vec::new();
    };
    let next = AtomicUsize::new(0);
    let outcomes = std::sync::Mutex::new(vec![None; peers.len()]);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::AcqRel);
                    let Some(peer) = peers.get(at) else {
                        break;
                    };
                    let outcome = copy_from_peer(
                        peer.as_ref(),
                        shards,
                        cache,
                        &plan.settings,
                        plan.pause,
                        progress,
                        plan.expired,
                    );
                    if let Ok(mut all) = outcomes.lock()
                        && let Some(slot) = all.get_mut(at)
                    {
                        *slot = Some(outcome);
                    }
                }
            });
        }
    });
    outcomes
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests;
