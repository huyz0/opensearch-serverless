//! What every worker shares, and what a parked or resumed connection carries.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use oss_mem::MemoryBudget;

use crate::admission::Permit;
use crate::body::BodyBuffers;
use crate::config::ServerConfig;
use crate::parked::SearchQueue;
use crate::router::Router;

/// State every worker shares.
pub(crate) struct Shared {
    pub(crate) config: ServerConfig,
    pub(crate) router: Router,
    pub(crate) budget: MemoryBudget,
    pub(crate) bodies: Arc<BodyBuffers>,
    pub(crate) queued: AtomicUsize,
    pub(crate) shutdown: AtomicBool,
    /// The search queue, when the server is configured with one.
    pub(crate) queue: Option<SearchQueue>,
}

impl Shared {
    /// Whether a connection or a resumed search waits for a worker.
    pub(crate) fn has_waiting(&self) -> bool {
        self.queued.load(Ordering::Acquire) > 0
            || self.queue.as_ref().is_some_and(SearchQueue::has_waiting)
    }
}

/// A request parked by the search queue: the bytes read of its connection,
/// from the first byte of its head, and the tenant it waits as.
pub(crate) struct ParkTicket {
    pub(crate) tenant: Arc<str>,
    pub(crate) bytes: Vec<u8>,
}

/// What a resumed connection starts with: its bytes and its permit.
pub(crate) struct Replay {
    pub(crate) bytes: Vec<u8>,
    pub(crate) permit: Permit,
}
