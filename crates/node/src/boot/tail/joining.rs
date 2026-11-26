//! The warm-up of a joining reader on its own thread: wait for peers to show
//! in the leases, copy their hot pages, and let the node in when enough is in
//! or the wait runs out.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use oss_cluster::Membership;
use oss_mem::{MemoryBudget, Pool};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;

use super::super::config::page_cache::WarmPeer;
use super::super::support::Held;
use crate::lifecycle::Readiness;
use crate::push_link::Leases;
use crate::search::SearchService;
use crate::warm_boot::{Join, finish_when_warm};
use crate::warm_copy::{Pause, Peer, Progress, Settings};
use crate::warm_join::{Plan, TransportPeer, peers_of, warm_from};
use oss_blob::Shared;

/// How long the leases are given to show a peer before the node is let in.
const PEERS_WAIT: Duration = Duration::from_secs(5);

/// How often a wait looks again.
const POLL: Duration = Duration::from_millis(100);

/// What a joining node's warm-up is made of.
pub(super) struct Joining {
    pub(super) warm: WarmPeer,
    pub(super) node_id: String,
    pub(super) client: Arc<Client>,
    pub(super) auth: Arc<Authenticator>,
    pub(super) membership: Arc<Membership>,
    pub(super) search: Arc<SearchService<Shared>>,
    pub(super) budget: MemoryBudget,
    pub(super) readiness: Arc<Readiness>,
}

/// Sleeps the thread.
struct Sleep;

impl Pause for Sleep {
    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Whether any other node's lease is shown.
fn others(leases: &Held, node_id: &str) -> bool {
    leases.current().iter().any(|l| l.node_id != node_id)
}

impl Joining {
    /// Runs the warm-up to its end; the node is ready when it returns.
    pub(super) fn run(self) {
        let started = Instant::now();
        let max_wait = Duration::from_secs(self.warm.max_wait_secs);
        let leases = Held(Arc::clone(&self.membership));
        // The first node of a cluster has nobody to ask: read the leases once
        // and let it in at once. A node that sees others waits for them to
        // show the search role, up to PEERS_WAIT.
        if self.membership.refresh().is_ok() && !others(&leases, &self.node_id) {
            self.readiness.finish_warming();
            return;
        }
        let mut peers = peers_of(&leases, &self.node_id);
        while peers.is_empty() && started.elapsed() < PEERS_WAIT.min(max_wait) {
            std::thread::sleep(POLL);
            peers = peers_of(&leases, &self.node_id);
        }
        if peers.is_empty() {
            self.readiness.finish_warming();
            return;
        }
        let list: Vec<Box<dyn Peer + Send + Sync>> = peers
            .iter()
            .map(|id| {
                Box::new(TransportPeer::new(
                    Arc::clone(&self.client),
                    Arc::clone(&self.auth),
                    Held(Arc::clone(&self.membership)),
                    id,
                )) as Box<dyn Peer + Send + Sync>
            })
            .collect();
        let progress = Progress::default();
        let done = AtomicBool::new(false);
        let expired = AtomicBool::new(false);
        let is_expired = || expired.load(Ordering::Acquire);
        let plan = Plan {
            settings: Settings {
                joiner: &self.node_id,
                max_shards: i32::try_from(self.warm.max_shards).unwrap_or(i32::MAX),
                max_bytes: i64::try_from(self.warm.max_bytes).unwrap_or(i64::MAX),
                bytes_per_second: self.warm.bytes_per_second,
                open_parallel: usize::try_from(self.warm.open_parallel).unwrap_or(1),
            },
            parallel_peers: usize::try_from(self.warm.parallel_peers).unwrap_or(1),
            pool: Pool::Transport,
            pause: &Sleep,
            expired: &is_expired,
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                warm_from(
                    &list,
                    &self.search,
                    self.search.page_cache(),
                    &self.budget,
                    &plan,
                    &progress,
                );
                done.store(true, Ordering::Release);
            });
            finish_when_warm(
                &self.readiness,
                &progress,
                &done,
                &Join {
                    ready_percent: self.warm.ready_percent,
                    max_wait,
                    poll: POLL,
                },
                &|| started.elapsed(),
                &Sleep,
                &expired,
            );
        });
    }
}
