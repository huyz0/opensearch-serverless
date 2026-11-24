//! What a node holds of the security configuration: one snapshot of the
//! five document sets, refreshed lazily when a request finds it older than
//! the interval ([ADR 0043](../../../docs/adr/0043-security-configuration-as-document-sets.md)).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, TryLockError};

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
    /// The generation counter's value before the read began.
    generation: u64,
}

struct Inner {
    retry_at: u64,
    marker: Vec<u8>,
    doc: Vec<u8>,
}

/// The snapshot and what it takes to refresh it.
pub struct SecurityState {
    sets: [(Kind, DocSet, DocSetCache); 5],
    /// The snapshot; read by every request, written by one refresh at a time.
    held: RwLock<Option<Held>>,
    /// The refresh's buffers and back-off; held while the stores are read.
    inner: Mutex<Inner>,
    /// Moves on every change made through this node.
    generation: AtomicU64,
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
            held: RwLock::new(None),
            inner: Mutex::new(Inner {
                retry_at: 0,
                // ALLOC: two buffers for the life of the node, buffer_bytes each, reserved above.
                marker: vec![0u8; buffer_bytes],
                // ALLOC: see the marker buffer.
                doc: vec![0u8; buffer_bytes],
            }),
            generation: AtomicU64::new(0),
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
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn held(&self) -> Option<(Arc<Config>, u64, u64)> {
        self.held
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|h| (Arc::clone(&h.config), h.taken, h.generation))
    }

    /// The configuration: the held copy while it is within the interval and
    /// no change made through this node is newer, else a fresh read (one
    /// marker GET a kind, and a GET for each changed document). One request
    /// reads while the others are served the held copy, unless a change
    /// through this node makes that copy wrong, when they wait for the read.
    /// A failed read keeps the last copy for three intervals and is retried
    /// after a second; with no copy younger than that the answer is
    /// [`Unavailable`], never a permissive default. A clock that has gone
    /// backwards since the copy was taken counts as the copy being old.
    ///
    /// # Errors
    ///
    /// [`Unavailable`].
    pub fn config(&self) -> Result<Arc<Config>, Unavailable> {
        let now = (self.clock)();
        let generation = self.generation.load(Ordering::SeqCst);
        let age = |taken: u64| now.checked_sub(taken);
        if let Some((config, taken, held_generation)) = self.held()
            && held_generation == generation
            && age(taken).is_some_and(|a| a < self.interval)
        {
            return Ok(config);
        }
        let mut inner = match self.inner.try_lock() {
            Ok(inner) => inner,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                // Another request is reading. A copy still within the stale
                // bound serves unless a change made here is newer than it.
                match self.held() {
                    Some((config, taken, held_generation))
                        if held_generation == generation
                            && age(taken).is_some_and(|a| a < self.stale_after()) =>
                    {
                        return Ok(config);
                    }
                    _ => self.lock(),
                }
            }
        };
        // Whoever held the lock may have read what this request needs.
        if let Some((config, taken, held_generation)) = self.held()
            && held_generation == generation
            && age(taken).is_some_and(|a| a < self.interval)
        {
            return Ok(config);
        }
        if now >= inner.retry_at {
            match self.read(&mut inner) {
                Ok(config) => {
                    let config = Arc::new(config);
                    *self
                        .held
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Held {
                        config: Arc::clone(&config),
                        taken: now,
                        generation,
                    });
                    return Ok(config);
                }
                Err(_) => inner.retry_at = now.saturating_add(RETRY_MILLIS),
            }
        }
        match self.held() {
            Some((config, taken, _)) if age(taken).is_some_and(|a| a < self.stale_after()) => {
                Ok(config)
            }
            _ => Err(Unavailable),
        }
    }

    fn stale_after(&self) -> u64 {
        self.interval.saturating_mul(STALE_INTERVALS)
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
