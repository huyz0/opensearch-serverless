//! What a node holds of the security configuration: one snapshot of the
//! five document sets, refreshed lazily when a request finds it older than
//! the interval ([ADR 0043](../../../docs/adr/0043-security-configuration-as-document-sets.md)).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use oss_blob::BlobStore;
use oss_coordinator::security::{Config, Kind};
use oss_doc::mapping::json::parse;
use oss_format::DocKind;
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_meta::{DocSet, DocSetCache, MetaError};

/// A snapshot older than this many intervals is not used.
pub const STALE_INTERVALS: u64 = 3;

/// What one kind of document is cached up to.
const CACHE_BYTES: usize = 1 << 20;

/// The longest a failed refresh waits before the next request tries again.
const RETRY_MILLIS: u64 = 1000;

/// The time, in milliseconds since any fixed point.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The configuration cannot be read and the last good copy is too old.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

struct Held {
    config: Arc<Config>,
    taken: u64,
}

struct Inner {
    held: Option<Held>,
    retry_at: u64,
    marker: Vec<u8>,
    doc: Vec<u8>,
}

/// The snapshot and what it takes to refresh it.
pub struct SecurityState {
    sets: [(Kind, DocSet, DocSetCache); 5],
    inner: Mutex<Inner>,
    stale: AtomicBool,
    interval: u64,
    admin: (String, String),
    clock: Clock,
    _reserved: Reservation,
}

fn doc_kind(kind: Kind) -> DocKind {
    match kind {
        Kind::Roles => DocKind::SecurityRoles,
        Kind::RolesMapping => DocKind::SecurityRolesMapping,
        Kind::InternalUsers => DocKind::SecurityInternalUsers,
        Kind::Tenants => DocKind::SecurityTenants,
        Kind::ActionGroups => DocKind::SecurityActionGroups,
    }
}

impl SecurityState {
    /// A state over the sets under `base`, refreshed when older than
    /// `interval_millis`, with `admin` (name and bcrypt hash) the static
    /// administrator. Its buffers and caches are reserved from the budget's
    /// request-body pool for the life of the state.
    ///
    /// # Errors
    ///
    /// A message when the base is refused or the pool cannot hold it.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
        interval_millis: u64,
        admin: (String, String),
        budget: &MemoryBudget,
        clock: Clock,
    ) -> Result<Self, String> {
        let reserved = budget
            .try_reserve(
                Pool::RequestBodies,
                buffer_bytes
                    .saturating_mul(2)
                    .saturating_add(CACHE_BYTES.saturating_mul(Kind::ALL.len())),
            )
            .map_err(|e| format!("security configuration memory: {e}"))?;
        let set = |kind: Kind| -> Result<(Kind, DocSet, DocSetCache), MetaError> {
            Ok((
                kind,
                DocSet::new(Arc::clone(store), base, doc_kind(kind))?,
                DocSetCache::with_budget(CACHE_BYTES),
            ))
        };
        let sets = [
            set(Kind::Roles),
            set(Kind::RolesMapping),
            set(Kind::InternalUsers),
            set(Kind::Tenants),
            set(Kind::ActionGroups),
        ];
        let [a, b, c, d, e] = sets;
        let sets = [a, b, c, d, e].map(|s| s.map_err(|e| e.to_string()));
        let [a, b, c, d, e] = sets;
        Ok(Self {
            sets: [a?, b?, c?, d?, e?],
            inner: Mutex::new(Inner {
                held: None,
                retry_at: 0,
                // ALLOC: two buffers for the life of the node, buffer_bytes each, reserved above.
                marker: vec![0u8; buffer_bytes],
                // ALLOC: see the marker buffer.
                doc: vec![0u8; buffer_bytes],
            }),
            stale: AtomicBool::new(false),
            interval: interval_millis,
            admin,
            clock,
            _reserved: reserved,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Marks the snapshot out of date: the next request reads the sets
    /// again. Called after a change made through this node.
    pub fn invalidate(&self) {
        self.stale.store(true, Ordering::SeqCst);
    }

    /// The configuration: the held copy while it is within the interval, else
    /// a fresh read (one marker GET a kind, and a GET for each changed
    /// document). A failed read keeps the last copy for three intervals and
    /// is retried by the next request after a second; with no copy younger
    /// than that the answer is [`Unavailable`], never a permissive default.
    ///
    /// # Errors
    ///
    /// [`Unavailable`].
    pub fn config(&self) -> Result<Arc<Config>, Unavailable> {
        let now = (self.clock)();
        let mut inner = self.lock();
        let forced = self.stale.load(Ordering::SeqCst);
        if let Some(held) = &inner.held
            && !forced
            && now.saturating_sub(held.taken) < self.interval
        {
            return Ok(Arc::clone(&held.config));
        }
        if now >= inner.retry_at || forced {
            match self.read(&mut inner) {
                Ok(config) => {
                    let config = Arc::new(config);
                    inner.held = Some(Held {
                        config: Arc::clone(&config),
                        taken: now,
                    });
                    self.stale.store(false, Ordering::SeqCst);
                    return Ok(config);
                }
                Err(_) => inner.retry_at = now.saturating_add(RETRY_MILLIS),
            }
        }
        match &inner.held {
            Some(held)
                if now.saturating_sub(held.taken)
                    < self.interval.saturating_mul(STALE_INTERVALS) =>
            {
                Ok(Arc::clone(&held.config))
            }
            _ => Err(Unavailable),
        }
    }

    fn read(&self, inner: &mut Inner) -> Result<Config, MetaError> {
        let Inner { marker, doc, .. } = inner;
        let mut config = Config::new().with_admin(&self.admin.0, &self.admin.1);
        for (kind, set, cache) in &self.sets {
            let snapshot = set.snapshot(cache, marker, doc)?;
            for (name, bytes) in snapshot.docs() {
                if let Ok(json) = parse(bytes) {
                    config.add(*kind, name, &json);
                }
            }
        }
        Ok(config)
    }
}
