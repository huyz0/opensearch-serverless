//! The node's side of the search queue (ADR 0056): which searches run now
//! and which wait off the workers.
//!
//! A search runs at once when its tenant runs fewer than its share of the
//! workers, or the node runs fewer than `workers - reserved` searches; so a
//! tenant over its share can take at most `workers - reserved` workers, and
//! a tenant under its share always finds the reserve. The REST server parks
//! the rest and offers them with [`SearchAdmission::try_reserve`], the same
//! rule. A tenant is the index named in `/{index}/_search`; a path with no
//! plain index name (an alias, a wildcard, a list, no index) is not counted.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_rest::{Decision, Method, Permit, QueueEvent, SearchAdmission};

/// How the queue is sized; `size` 0 is no queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The most searches parked.
    pub size: usize,
    /// The share of the workers one tenant's searches may hold, as a
    /// percentage.
    pub share_pct: usize,
    /// The workers searches over their share leave to the rest.
    pub reserved: usize,
    /// How long a parked search waits, in milliseconds.
    pub wait_millis: u64,
    /// `OSS_HTTP_MAX_CONNECTIONS`: open connections, served and waiting.
    pub max_connections: u64,
}

/// What the queue has done, for the usage route.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Searches running that the queue counts.
    pub running: usize,
    /// Searches parked now.
    pub parked: usize,
    /// Searches parked since the start.
    pub deferred: u64,
    /// Parked searches that resumed.
    pub resumed: u64,
    /// Searches refused with 429 because the queue was full.
    pub refused: u64,
    /// Parked searches answered 429 at the wait.
    pub expired: u64,
    /// Parked searches dropped because the peer closed.
    pub dropped: u64,
    /// The longest a parked search waited, in milliseconds.
    pub longest_wait_millis: u64,
}

#[derive(Default)]
struct State {
    running: usize,
    /// Tenant to searches running; gone at 0.
    tenants: HashMap<Arc<str>, usize>,
}

/// The node's [`SearchAdmission`].
pub struct QueueAdmission {
    /// A tenant's searches that run at once whatever the node runs.
    share: usize,
    /// Searches the node runs before an over-share tenant must wait.
    free_below: usize,
    state: Mutex<State>,
    deferred: AtomicU64,
    resumed: AtomicU64,
    refused: AtomicU64,
    expired: AtomicU64,
    dropped: AtomicU64,
    longest: AtomicU64,
}

/// A running search the queue counts; dropped when its response is done.
struct Running {
    queue: Arc<QueueAdmission>,
    tenant: Arc<str>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut s = self.queue.lock();
        s.running = s.running.saturating_sub(1);
        if let Some(n) = s.tenants.get_mut(&self.tenant) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                s.tenants.remove(&self.tenant);
            }
        }
    }
}

/// The index of `/{index}/_search`, when it is one plain name.
fn tenant_of(path: &str) -> Option<&str> {
    let (index, tail) = path.strip_prefix('/')?.split_once('/')?;
    let plain =
        !index.is_empty() && !index.starts_with(['_', '.']) && !index.contains([',', '*', '?']);
    (plain && tail == "_search").then_some(index)
}

impl QueueAdmission {
    /// A queue over `workers` REST workers.
    pub fn new(config: &Config, workers: usize) -> Arc<Self> {
        let share = workers
            .saturating_mul(config.share_pct)
            .div_ceil(100)
            .max(1);
        Arc::new(Self {
            share,
            free_below: workers.saturating_sub(config.reserved),
            state: Mutex::new(State::default()),
            deferred: AtomicU64::new(0),
            resumed: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            longest: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The one rule for both a new search and an offer: a permit when the
    /// tenant may run now.
    fn reserve(self: &Arc<Self>, tenant: &str) -> Option<Permit> {
        let mut s = self.lock();
        let own = s.tenants.get(tenant).copied().unwrap_or(0);
        if own >= self.share && s.running >= self.free_below {
            return None;
        }
        s.running = s.running.saturating_add(1);
        let key: Arc<str> = match s.tenants.get_key_value(tenant) {
            Some((k, _)) => Arc::clone(k),
            None => Arc::from(tenant), // ALLOC: one key a tenant while it runs.
        };
        let n = s.tenants.entry(Arc::clone(&key)).or_insert(0);
        *n = n.saturating_add(1);
        Some(Box::new(Running {
            queue: Arc::clone(self),
            tenant: key,
        }))
    }

    /// What the queue has done.
    pub fn report(&self) -> Report {
        let deferred = self.deferred.load(Ordering::Relaxed);
        let left = self
            .resumed
            .load(Ordering::Relaxed)
            .saturating_add(self.expired.load(Ordering::Relaxed))
            .saturating_add(self.dropped.load(Ordering::Relaxed));
        Report {
            running: self.lock().running,
            parked: usize::try_from(deferred.saturating_sub(left)).unwrap_or(usize::MAX),
            deferred,
            resumed: self.resumed.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            longest_wait_millis: self.longest.load(Ordering::Relaxed),
        }
    }
}

/// The hook the router carries: the queue's state behind an `Arc`, since a
/// permit holds the queue it returns to.
pub struct Hook(pub Arc<QueueAdmission>);

impl SearchAdmission for Hook {
    fn decide(&self, _method: Option<Method>, path: &str) -> Decision {
        let Some(tenant) = tenant_of(path) else {
            return Decision::Run(None);
        };
        match self.0.reserve(tenant) {
            Some(permit) => Decision::Run(Some(permit)),
            None => Decision::Defer(Arc::from(tenant)), // ALLOC: one key a parked search.
        }
    }

    fn try_reserve(&self, tenant: &str) -> Option<Permit> {
        self.0.reserve(tenant)
    }

    fn note(&self, event: QueueEvent) {
        let q = &self.0;
        match event {
            QueueEvent::Deferred => q.deferred.fetch_add(1, Ordering::Relaxed),
            QueueEvent::Resumed(waited) => {
                q.longest.fetch_max(waited, Ordering::Relaxed);
                q.resumed.fetch_add(1, Ordering::Relaxed)
            }
            QueueEvent::Refused => q.refused.fetch_add(1, Ordering::Relaxed),
            QueueEvent::Expired => q.expired.fetch_add(1, Ordering::Relaxed),
            QueueEvent::Dropped => q.dropped.fetch_add(1, Ordering::Relaxed),
        };
    }
}

impl crate::assemble::Assembled {
    /// Lets the REST server park searches over their tenant's share in
    /// `admission`'s queue (ADR 0056).
    #[must_use]
    pub fn with_search_queue(
        mut self,
        admission: Arc<crate::search_queue::QueueAdmission>,
    ) -> Self {
        self.router = self
            .router
            .with_search_admission(Arc::new(crate::search_queue::Hook(Arc::clone(&admission))));
        self.search_queue = Some(admission);
        self
    }
}

/// Installs the queue on `assembled` when `config` asks for one.
pub(crate) fn install(
    assembled: crate::assemble::Assembled,
    config: &crate::boot::NodeConfig,
) -> crate::assemble::Assembled {
    if config.search_queue.size == 0 {
        return assembled;
    }
    let workers = oss_rest::ServerConfig::default().workers;
    assembled.with_search_queue(QueueAdmission::new(&config.search_queue, workers))
}

impl crate::boot::NodeConfig {
    /// The REST server's configuration for this node.
    pub(crate) fn rest_config(&self) -> oss_rest::ServerConfig {
        let q = &self.search_queue;
        oss_rest::ServerConfig {
            max_connections: usize::try_from(q.max_connections).unwrap_or(usize::MAX),
            search_queue_size: q.size,
            search_queue_wait: std::time::Duration::from_millis(q.wait_millis),
            ..oss_rest::ServerConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(workers: usize, share_pct: usize, reserved: usize) -> Arc<QueueAdmission> {
        QueueAdmission::new(
            &Config {
                size: 100,
                share_pct,
                reserved,
                wait_millis: 1000,
                max_connections: 1024,
            },
            workers,
        )
    }

    #[test]
    fn a_tenant_is_the_one_plain_index_of_a_search_path() {
        assert_eq!(tenant_of("/logs/_search"), Some("logs"));
        for no in [
            "/_search",
            "/_all/_search",
            "/a,b/_search",
            "/a*/_search",
            "/.hidden/_search",
            "/logs/_count",
            "/logs/_search/template",
            "logs/_search",
        ] {
            assert_eq!(tenant_of(no), None, "{no}");
        }
    }

    // The offer and a new search take the same rule: a tenant over its
    // share runs while the node is under `workers - reserved`.
    #[test]
    fn a_tenant_over_its_share_runs_while_the_node_is_under_its_free_bound_and_then_waits() {
        let q = queue(10, 20, 2); // share 2, free below 8
        let mut held = Vec::new();
        for _ in 0..8 {
            held.push(q.reserve("noisy").expect("under the free bound"));
        }
        assert!(
            q.reserve("noisy").is_none(),
            "over its share, node at the bound"
        );
        let quiet = q
            .reserve("quiet")
            .expect("under its share finds the reserve");
        assert_eq!(q.report().running, 9);
        drop(held.pop());
        assert!(
            q.reserve("noisy").is_none(),
            "8 running is the bound, not under it"
        );
        drop(held.pop());
        assert!(
            q.reserve("noisy").is_some(),
            "7 running is under it: the offer and admission agree"
        );
        drop(quiet);
    }

    #[test]
    fn permits_return_the_counts_and_the_tenant_leaves_the_map() {
        let q = queue(4, 25, 1);
        let a = q.reserve("a").expect("runs");
        let b = q.reserve("a").expect("runs");
        assert_eq!(q.report().running, 2);
        drop(a);
        drop(b);
        assert_eq!(q.report().running, 0);
        assert!(q.lock().tenants.is_empty());
    }

    #[test]
    fn the_report_counts_what_the_server_tells_it() {
        let q = queue(4, 25, 1);
        let hook = Hook(Arc::clone(&q));
        hook.note(QueueEvent::Deferred);
        hook.note(QueueEvent::Deferred);
        hook.note(QueueEvent::Resumed(40));
        hook.note(QueueEvent::Refused);
        let r = q.report();
        assert_eq!((r.deferred, r.parked, r.resumed, r.refused), (2, 1, 1, 1));
        assert_eq!(r.longest_wait_millis, 40);
    }
}
