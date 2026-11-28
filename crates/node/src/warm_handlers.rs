//! The transport handlers of the warm-copy actions on a reader: the hot-set
//! listing and the page serving of [`crate::warm_peer`], behind the node's
//! message check, with the joiner placed among the readers of each shard,
//! the sizes bounded and the requests served at once capped.
//!
//! The actions are not behind a switch of their own on the peer; a node
//! answers them only when its settings install these handlers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use lucene_store::page_cache::PageCache;
use oss_cluster::placement::{ROLE_SEARCH, ShardKey, readers_for};
use oss_format::NodeLease;
use oss_transport::actions::tail::ShardRef;
use oss_transport::actions::warm::{HotSetRequest, PagesRequest};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::push_link::Leases;
use crate::warm_peer::{HeldShard, hot_set, serve};
use crate::warm_rate::ServeRate;

/// The most a hot-set ask takes: a name, a parent task, two numbers.
const MAX_HOT_SET_ASK_BYTES: usize = 4096;

/// The most a page ask takes: at most 64 keys of a few hundred bytes.
const MAX_PAGES_ASK_BYTES: usize = 64 << 10;

/// The shards this node holds open as a reader.
pub trait HeldShards: Send + Sync {
    /// The shards held now, as the page cache's keys name them.
    fn held(&self) -> Vec<HeldShard>;
}

/// What bounds a peer's answers.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// How many of a shard's first readers a joiner must be among for the
    /// shard to be listed for it.
    pub width: usize,
    /// The most shards an answer names, whatever the ask says.
    pub max_shards: usize,
    /// The most stored bytes an answer names, whatever the ask says.
    pub max_bytes: u64,
    /// The most warm requests this node serves at once.
    pub max_concurrent: usize,
}

/// A count of the requests being served, up to a limit, and the rate the
/// bytes of page batches go out at.
#[derive(Debug, Default)]
pub struct Gate {
    serving: AtomicUsize,
    rate: ServeRate,
}

/// One request being served, counted until it drops.
struct Permit<'a>(&'a AtomicUsize);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Gate {
    /// A gate that also holds page batches to at most `bytes_per_second`
    /// across every joiner; 0 holds none.
    #[must_use]
    pub fn with_rate(bytes_per_second: u64) -> Self {
        Self {
            serving: AtomicUsize::new(0),
            rate: ServeRate::new(bytes_per_second),
        }
    }

    /// A gate whose rate also follows the node's search latency (`pressure`),
    /// up to `bytes_per_second`.
    #[must_use]
    pub fn with_pressure(
        bytes_per_second: u64,
        pressure: Arc<crate::warm_pressure::Pressure>,
    ) -> Self {
        Self {
            serving: AtomicUsize::new(0),
            rate: ServeRate::adaptive(bytes_per_second, pressure),
        }
    }

    fn enter(&self, limit: usize) -> Option<Permit<'_>> {
        let before = self.serving.fetch_add(1, Ordering::AcqRel);
        if before >= limit {
            self.serving.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Permit(&self.serving))
    }
}

fn busy() -> RemoteException {
    RemoteException::rejected("too many warm-copy requests at once")
}

/// Answers `internal:serverless/warm/hot_set`.
pub struct HotSetHandler<L, H> {
    cache: Arc<PageCache>,
    held: H,
    leases: L,
    limits: Limits,
    gate: Arc<Gate>,
}

impl<L: Leases, H: HeldShards> HotSetHandler<L, H> {
    /// A handler listing `cache`'s hot pages of the shards `held` names,
    /// placing a joiner by `leases`, sharing `gate` with the page handler.
    pub fn new(cache: Arc<PageCache>, held: H, leases: L, limits: Limits, gate: Arc<Gate>) -> Self {
        Self {
            cache,
            held,
            leases,
            limits,
            gate,
        }
    }
}

/// The leases with `joiner` in them as a reader, whatever it says of itself.
fn with_joiner(leases: &[NodeLease], joiner: &str) -> Vec<NodeLease> {
    // ALLOC: one lease per member of the fleet, once per ask.
    let mut all: Vec<NodeLease> = leases
        .iter()
        .filter(|l| l.node_id != joiner)
        .cloned()
        .collect();
    let mut lease = NodeLease::new(joiner, "", "", i64::MAX);
    lease.set_roles([ROLE_SEARCH.to_owned()]);
    all.push(lease);
    all
}

impl<L: Leases, H: HeldShards> Handler for HotSetHandler<L, H> {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        if request.message.len() > MAX_HOT_SET_ASK_BYTES {
            return Err(RemoteException::status(
                "warm hot-set ask too large",
                "BAD_REQUEST",
            ));
        }
        let ask = HotSetRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed warm hot-set ask", "BAD_REQUEST"))?;
        let _permit = self
            .gate
            .enter(self.limits.max_concurrent)
            .ok_or_else(busy)?;
        let leases = with_joiner(&self.leases.current(), &ask.joiner);
        let serves = |shard: &ShardRef| {
            let key = ShardKey::new(&shard.index_name, shard.shard);
            readers_for(key, leases.iter(), ROLE_SEARCH, self.limits.width)
                .contains(&ask.joiner.as_str())
        };
        let max_shards = usize::try_from(ask.max_shards)
            .unwrap_or(0)
            .min(self.limits.max_shards);
        let max_bytes = u64::try_from(ask.max_bytes)
            .unwrap_or(0)
            .min(self.limits.max_bytes);
        let held = self.held.held();
        hot_set(&self.cache, &held, serves, max_shards, max_bytes)
            .write(out)
            .map_err(|_| RemoteException::status("warm hot-set answer", "INTERNAL_SERVER_ERROR"))
    }
}

/// Answers `internal:serverless/warm/pages`.
pub struct PagesHandler<H> {
    cache: Arc<PageCache>,
    held: H,
    limits: Limits,
    gate: Arc<Gate>,
}

impl<H: HeldShards> PagesHandler<H> {
    /// A handler serving pages from `cache` of the shards `held` names.
    pub fn new(cache: Arc<PageCache>, held: H, limits: Limits, gate: Arc<Gate>) -> Self {
        Self {
            cache,
            held,
            limits,
            gate,
        }
    }
}

impl<H: HeldShards> Handler for PagesHandler<H> {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        if request.message.len() > MAX_PAGES_ASK_BYTES {
            return Err(RemoteException::status(
                "warm pages ask too large",
                "BAD_REQUEST",
            ));
        }
        let ask = PagesRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed warm pages ask", "BAD_REQUEST"))?;
        let _permit = self
            .gate
            .enter(self.limits.max_concurrent)
            .ok_or_else(busy)?;
        let held = self.held.held();
        let answer = serve(&self.cache, &held, &ask)
            .map_err(|_| RemoteException::status("a key outside the held shards", "BAD_REQUEST"))?;
        // The wait is taken holding the permit above, so the batches queued
        // behind this one are at most the node's `max_concurrent`.
        let bytes: u64 = answer.pages.iter().map(|p| p.bytes.len() as u64).sum();
        let wait = self.gate.rate.reserve(Instant::now(), bytes);
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        answer
            .write(out)
            .map_err(|_| RemoteException::status("warm pages answer", "INTERNAL_SERVER_ERROR"))
    }
}

#[cfg(test)]
mod tests;
