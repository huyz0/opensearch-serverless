//! The sender alerting and ISM call in process: a config by id, through a
//! per-node cache, delivered with the same `Senders` the test route uses.
//!
//! A config is used for `ttl`; the first send that finds it older reads the
//! configs set's marker (one GET) and reads the config again only when the
//! set moved. One flight a config at a time, so a burst past the TTL costs
//! one refresh. Nothing polls and nothing runs on a timer, so an idle node
//! makes no request. What the cache holds, and every buffer a refresh reads
//! into, is charged to the open-indexes pool before it is allocated.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_meta::DocSet;
use serde_json::Value;

use super::deliver::Delivery;
use super::send::Senders;

/// The buffer a marker is read into: the most a set's marker can be.
const MARKER_BYTES: usize = oss_format::MAX_DOC_SET_MARKER_BYTES;
/// What a parsed config costs to keep, in document bytes.
const RETAINED_FACTOR: usize = 4;
/// The most configs a node keeps.
const MAX_ENTRIES: usize = 1024;

struct Entry {
    config: Arc<Value>,
    /// The configs set's change count when this was read.
    version: i64,
    checked: Instant,
    _held: Reservation,
}

/// What a node's notifier has counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotifierCounts {
    /// Sends asked of it.
    pub sends: u64,
    /// Sends it could not attempt for lack of memory or a failed read.
    pub skipped: u64,
    /// Configs held now.
    pub cached: usize,
}

/// Sends a message to a config by id.
pub struct Notifier {
    store: Arc<dyn BlobStore>,
    base: String,
    budget: MemoryBudget,
    senders: Arc<Senders>,
    ttl: Duration,
    entries: Mutex<HashMap<String, Entry>>,
    flights: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    sends: AtomicU64,
    skipped: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Notifier {
    /// A notifier over the configs under `base` in `store`, keeping a config
    /// for `ttl`.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        budget: MemoryBudget,
        senders: Arc<Senders>,
        ttl: Duration,
    ) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            budget,
            senders,
            ttl,
            entries: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            sends: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
        }
    }

    /// What this notifier has counted and holds.
    pub fn counts(&self) -> NotifierCounts {
        NotifierCounts {
            sends: self.sends.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            cached: lock(&self.entries).len(),
        }
    }

    /// Sends `text` through the config `config_id`. A config that is gone,
    /// disabled or of a type this node does not send to is a failed delivery,
    /// never a panic or an error the caller must handle.
    pub fn send(&self, config_id: &str, text: &str) -> Delivery {
        self.send_with_title(config_id, "Notification", text)
    }

    /// As [`Notifier::send`], with the title an email or SNS subject carries.
    pub fn send_with_title(&self, config_id: &str, title: &str, text: &str) -> Delivery {
        self.sends.fetch_add(1, Ordering::Relaxed);
        let config = match self.config(config_id) {
            Ok(Some(c)) => c,
            Ok(None) => return Delivery::failed(404, "not found"),
            Err(skip) => {
                self.skipped.fetch_add(1, Ordering::Relaxed);
                return skip;
            }
        };
        if config["is_enabled"].as_bool() != Some(true) {
            return Delivery::failed(403, "disabled");
        }
        let resolve = |id: &str| self.config(id).ok().flatten();
        self.senders.send(&config, title, text, &resolve).summary()
    }

    /// The config held fresh, or the one a refresh read.
    fn config(&self, id: &str) -> Result<Option<Arc<Value>>, Delivery> {
        if let Some(c) = self.fresh(id) {
            return Ok(Some(c));
        }
        let flight = Arc::clone(lock(&self.flights).entry(id.to_owned()).or_default());
        let out = {
            let _one = lock(&flight);
            // Another send may have refreshed it while this one waited.
            match self.fresh(id) {
                Some(c) => Ok(Some(c)),
                None => self.refresh(id),
            }
        };
        let mut flights = lock(&self.flights);
        // The map and this send hold it: nobody else is waiting.
        if Arc::strong_count(&flight) <= 2 {
            flights.remove(id);
        }
        out
    }

    fn fresh(&self, id: &str) -> Option<Arc<Value>> {
        lock(&self.entries)
            .get(id)
            .filter(|e| e.checked.elapsed() < self.ttl)
            .map(|e| Arc::clone(&e.config))
    }

    fn refresh(&self, id: &str) -> Result<Option<Arc<Value>>, Delivery> {
        let skip = || Delivery::failed(503, "skipped");
        let kind = DocKind::NotificationConfigs;
        let doc_bytes = kind.max_doc_bytes();
        let set = DocSet::new(Arc::clone(&self.store), &self.base, kind).map_err(|_| skip())?;
        let _reading = self
            .budget
            .try_reserve(Pool::OpenIndexes, MARKER_BYTES.saturating_add(doc_bytes))
            .map_err(|_| skip())?;
        // ALLOC: one marker buffer, reserved above.
        let mut marker = vec![0u8; MARKER_BYTES];
        let version = set.version(&mut marker).map_err(|_| skip())?;
        {
            let mut entries = lock(&self.entries);
            if let Some(e) = entries.get_mut(id)
                && e.version == version
            {
                e.checked = Instant::now();
                return Ok(Some(Arc::clone(&e.config)));
            }
        }
        // ALLOC: one document buffer, reserved above.
        let mut doc = vec![0u8; doc_bytes];
        let read = set.get(id, &mut marker, &mut doc).map_err(|_| skip())?;
        let Some(len) = read else {
            lock(&self.entries).remove(id);
            return Ok(None);
        };
        let stored: Value = serde_json::from_slice(doc.get(..len).unwrap_or_default())
            .map_err(|_| Delivery::failed(500, "unreadable"))?;
        let held = self
            .budget
            .try_reserve(Pool::OpenIndexes, len.saturating_mul(RETAINED_FACTOR))
            .map_err(|_| skip())?;
        let config = Arc::new(stored["config"].clone());
        let mut entries = lock(&self.entries);
        if entries.len() >= MAX_ENTRIES
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, e)| e.checked)
                .map(|(k, _)| k.clone())
        {
            entries.remove(&oldest);
        }
        entries.insert(
            id.to_owned(),
            Entry {
                config: Arc::clone(&config),
                version,
                checked: Instant::now(),
                _held: held,
            },
        );
        Ok(Some(config))
    }
}
