//! The tail overlay as an assembled node holds it: the writer side that
//! keeps and pushes acknowledged writes and answers fetches, the reader
//! side that fetches and holds them, and the transport handlers for both.
//!
//! Built only under `serverless.search.tail_overlay.enabled`; off, nothing
//! here exists and the write path and searches are as they were.

use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_format::WalRecord;
use oss_transport::actions::tail::{FETCH_ACTION, PUSH_ACTION};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;

use crate::handlers::{Duplicate, Registry};
use crate::newest_ask::{LiveWriter, LiveWriters};
use crate::overlay_builder::TailFetcher;
use crate::push_link::Leases;
use crate::tail_keep::TailKeep;
use crate::tail_link::{TailFetchHandler, TransportTailLink};
use crate::tail_push::{TailLink, TailPush};
use crate::tail_reader_link::TransportTailFetch;
use crate::tail_serve::{ShardFacts, TailServe};
use crate::writer::{TailSink, WriterRole};

/// The nodes a node's leases show live: unexpired and not revoked.
pub struct LeaseWriters<L> {
    leases: L,
    clock: Arc<dyn LeaseClock>,
}

impl<L: Leases> LeaseWriters<L> {
    /// Writers found in `leases`, expiry judged by `clock`.
    pub fn new(leases: L, clock: Arc<dyn LeaseClock>) -> Self {
        Self { leases, clock }
    }
}

impl<L: Leases> LiveWriters for LeaseWriters<L> {
    fn live(&self, node_id: &str) -> Option<LiveWriter> {
        let now = self.clock.now_millis();
        let leases = self.leases.current();
        let lease = leases
            .iter()
            .find(|l| l.node_id == node_id && !l.revoked && l.expires_at_millis > now)?;
        Some(LiveWriter {
            address: lease.address.parse::<SocketAddr>().ok()?,
            // ALLOC: one id a lookup.
            ephemeral_id: lease.ephemeral_id.clone(),
        })
    }
}

/// What the node supplies to turn the tail overlay on.
pub struct TailParts {
    /// `serverless.search.tail_overlay.enabled`.
    pub enabled: bool,
    /// This node's id, the name readers subscribe under.
    pub node_id: String,
    /// Signs and verifies the tail actions.
    pub auth: Arc<Authenticator>,
    /// Asks a shard's writer for the records past a commit.
    pub fetcher: Arc<dyn TailFetcher>,
    /// Sends a push to a reader.
    pub link: Arc<dyn TailLink>,
    /// Counts the milliseconds grants and holds are judged in.
    pub clock: Arc<dyn LeaseClock>,
    /// Whether this node's ownership of its shards is in doubt.
    pub in_doubt: Arc<dyn Fn() -> bool + Send + Sync>,
    /// How often queued pushes are flushed.
    pub flush_every: Duration,
}

impl TailParts {
    /// Parts calling through `client` and finding peers in `leases`.
    pub fn over_transport<L: Leases + Clone + 'static>(
        enabled: bool,
        node_id: &str,
        client: Arc<Client>,
        auth: Arc<Authenticator>,
        leases: L,
        clock: Arc<dyn LeaseClock>,
    ) -> Self {
        Self {
            enabled,
            // ALLOC: one id, once.
            node_id: node_id.to_owned(),
            fetcher: Arc::new(TransportTailFetch::new(
                Arc::clone(&client),
                Arc::clone(&auth),
                LeaseWriters::new(leases.clone(), Arc::clone(&clock)),
            )),
            link: Arc::new(TransportTailLink::new(client, Arc::clone(&auth), leases)),
            auth,
            clock,
            in_doubt: Arc::new(|| false),
            flush_every: Duration::from_millis(10),
        }
    }
}

struct RoleFacts {
    role: Arc<WriterRole>,
    in_doubt: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ShardFacts for RoleFacts {
    fn writes(&self, uuid: &str, shard: u32) -> bool {
        self.role.held(uuid, shard)
    }

    fn ownership_in_doubt(&self) -> bool {
        (self.in_doubt)()
    }

    fn overlay_on(&self, _: &str, _: u32) -> bool {
        // Only built with the setting on.
        true
    }

    fn max_seq_no(&self, uuid: &str, shard: u32) -> i64 {
        self.role.max_seq_no(uuid, shard)
    }
}

struct Sink {
    serve: Arc<TailServe>,
    keep: Arc<TailKeep>,
    push: Arc<TailPush>,
}

impl TailSink for Sink {
    fn acked(&self, index: &str, uuid: &str, shard: u32, records: &[WalRecord<'static>]) {
        self.push.acked(index, uuid, shard, records, true);
    }

    fn published(&self, uuid: &str, shard: u32, checkpoint: i64) {
        self.keep.published(uuid, shard, checkpoint);
    }

    fn released(&self, uuid: &str, shard: u32) {
        self.serve.released(uuid, shard);
    }
}

/// Flushes queued pushes every interval until dropped.
struct FlushTimer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl FlushTimer {
    fn spawn(push: Arc<TailPush>, every: Duration) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("tail-flush".to_owned())
            .spawn(move || {
                let (flag, wake) = &*signal;
                loop {
                    let stopped = flag.lock().unwrap_or_else(PoisonError::into_inner);
                    let (stopped, _) = wake
                        .wait_timeout_while(stopped, every, |stopped| !*stopped)
                        .unwrap_or_else(PoisonError::into_inner);
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    push.flush();
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for FlushTimer {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            // A flush that panicked already ended the thread.
            let _ = thread.join();
        }
    }
}

/// What a node holds while the tail overlay is on.
pub struct TailNode {
    /// The reader side: held records, the builder and its push handler.
    pub wired: crate::tail_overlay::Wired,
    /// The writer's keep.
    pub keep: Arc<TailKeep>,
    _timer: FlushTimer,
}

/// Why the tail overlay was not wired.
#[derive(Debug)]
pub enum TailError {
    /// A tail action already has a handler.
    Handler(Duplicate),
}

/// Wires both sides over `service` and `role` with `budget`'s
/// `open_indexes` pool. `None` when off or already wired.
///
/// # Errors
///
/// [`TailError::Handler`] when an action is registered twice.
pub fn wire<S: oss_blob::BlobStore + 'static>(
    service: &crate::search::SearchService<S>,
    role: &Arc<WriterRole>,
    budget: oss_mem::MemoryBudget,
    parts: TailParts,
) -> Result<Option<(TailNode, Registry)>, TailError> {
    if !parts.enabled {
        return Ok(None);
    }
    let clock = Arc::clone(&parts.clock);
    let reader_clock: crate::tail_reader_link::Clock = Arc::new(move || clock.now_millis());
    let Some(wired) = crate::tail_overlay::wire(
        service,
        true,
        budget,
        parts.fetcher,
        parts.node_id,
        Arc::clone(&reader_clock),
    ) else {
        return Ok(None);
    };
    let keep = Arc::new(TailKeep::new());
    let facts: Arc<dyn ShardFacts> = Arc::new(RoleFacts {
        role: Arc::clone(role),
        in_doubt: parts.in_doubt,
    });
    let serve = Arc::new(TailServe::new(
        Arc::clone(&keep),
        Arc::clone(&facts),
        parts.clock,
    ));
    let push = Arc::new(TailPush::new(
        Arc::clone(&keep),
        Arc::clone(&serve),
        parts.link,
        facts,
    ));
    let mut registry = Registry::new(parts.auth);
    registry
        .add(FETCH_ACTION, TailFetchHandler::new(Arc::clone(&serve)))
        .map_err(TailError::Handler)?;
    registry
        .add(
            PUSH_ACTION,
            crate::tail_reader_link::TailPushHandler::new(Arc::clone(&wired.held), reader_clock),
        )
        .map_err(TailError::Handler)?;
    role.set_tail_sink(Arc::new(Sink {
        serve,
        keep: Arc::clone(&keep),
        push: Arc::clone(&push),
    }));
    let timer = FlushTimer::spawn(push, parts.flush_every);
    Ok(Some((
        TailNode {
            wired,
            keep,
            _timer: timer,
        },
        registry,
    )))
}
