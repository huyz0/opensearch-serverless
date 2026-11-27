//! The node's model cache: what an `sltr` rescore resolves a model from.
//!
//! A model is read from its set as the plugin's put stored it (the feature
//! set embedded) and kept parsed, keyed by store and name. Freshness is by
//! time and one read: an entry is used for `ttl`, then the first search that
//! finds it older reads the models set's marker (one GET) and, only when the
//! set moved, reads the model again. Nothing polls, so an idle store makes no
//! request. What the cache holds is charged to the open-indexes pool before
//! it is allocated and never passes `bytes`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_meta::{DocSet, MetaError};
use oss_reader::ltr::Evaluator;
use oss_reader::ltr::models::{Feature, FeatureSet, Models, ResolveError, Resolved};
use serde_json::Value;

use super::DEFAULT;

/// What a parsed model costs to keep, in definition bytes.
const RETAINED_FACTOR: usize = 4;

/// The cache's bounds.
pub struct ModelCache {
    /// The buffer a marker is read into.
    pub buffer_bytes: usize,
    /// `serverless.ltr.max_window`.
    pub max_window: usize,
    /// `serverless.ltr.cache_ttl`.
    pub ttl: Duration,
    /// `serverless.ltr.cache_bytes`.
    pub bytes: usize,
}

/// Which set an object is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Which {
    Model,
    Features,
}

impl Which {
    fn slot(self) -> usize {
        match self {
            Self::Model => 0,
            Self::Features => 1,
        }
    }

    fn kind(self) -> DocKind {
        match self {
            Self::Model => DocKind::LtrModels,
            Self::Features => DocKind::LtrFeatureSets,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Features => "feature set",
        }
    }
}

/// What a read parsed to.
#[derive(Clone)]
enum Loaded {
    Model(Arc<Resolved>),
    Features(Arc<FeatureSet>),
}

struct Entry {
    resolved: Loaded,
    /// The models set's change count when this was read.
    version: i64,
    checked: Instant,
    used: u64,
    _held: Reservation,
    bytes: usize,
}

/// What one kind of cache has counted since the node started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheCounts {
    /// Resolutions answered from the cache, fresh or after a marker check.
    pub hits: u64,
    /// Resolutions that read the object.
    pub misses: u64,
    /// Entries dropped to make room.
    pub evictions: u64,
    /// Entries held now.
    pub entries: usize,
    /// Bytes those entries are charged.
    pub bytes: usize,
}

/// The node's cache counts and the resolutions it served.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeStats {
    /// The model cache.
    pub model: CacheCounts,
    /// The feature set cache.
    pub featureset: CacheCounts,
    /// Resolutions asked of the cache.
    pub requests: u64,
    /// Resolutions that failed.
    pub errors: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<(Which, String, String), Entry>,
    hits: [u64; 2],
    misses: [u64; 2],
    evictions: [u64; 2],
    requests: u64,
    errors: u64,
    held: usize,
    tick: u64,
}

/// Resolves models over the store, through the cache.
pub struct StoreModels {
    store: Arc<dyn BlobStore>,
    base: String,
    cache: ModelCache,
    budget: MemoryBudget,
    state: Mutex<State>,
}

impl StoreModels {
    /// A resolver over the stores under `base` in `store`.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        cache: ModelCache,
        budget: MemoryBudget,
    ) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            cache,
            budget,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set(&self, store: &str, which: Which) -> Result<DocSet, ResolveError> {
        // ALLOC: one key prefix, once a load.
        let base = format!("{}ltr/{store}/", self.base);
        DocSet::new_created(Arc::clone(&self.store), &base, which.kind())
            .map_err(|e| ResolveError::Failed(e.to_string()))
    }

    /// Reads and parses the model, charging what it will hold first.
    fn load(
        &self,
        which: Which,
        store: &str,
        name: &str,
    ) -> Result<(Loaded, i64, Reservation, usize), ResolveError> {
        let set = self.set(store, which)?;
        let missing = || {
            ResolveError::NotFound(format!(
                "{} [{name}] does not exist in the store [{store}]",
                which.noun()
            ))
        };
        let max = which.kind().max_doc_bytes();
        let _read = self
            .budget
            .try_reserve(
                Pool::OpenIndexes,
                max.saturating_add(self.cache.buffer_bytes),
            )
            .map_err(|e| ResolveError::TooBig(e.to_string()))?;
        // ALLOC: one marker and one document buffer a load, reserved above.
        let mut marker = vec![0u8; self.cache.buffer_bytes];
        let version = set.version(&mut marker).map_err(|e| match e {
            MetaError::NoSuchSet => ResolveError::NotFound(format!("no such store [{store}]")),
            other => ResolveError::Failed(other.to_string()),
        })?;
        // ALLOC: see above.
        let mut doc = vec![0u8; max];
        let len = set
            .get(name, &mut marker, &mut doc)
            .map_err(|e| ResolveError::Failed(e.to_string()))?
            .ok_or_else(missing)?;
        let bytes = len.saturating_mul(RETAINED_FACTOR);
        if bytes > self.cache.bytes {
            return Err(ResolveError::TooBig(format!(
                "model [{name}] needs {bytes} bytes, over serverless.ltr.cache_bytes [{}]",
                self.cache.bytes
            )));
        }
        let held = self
            .budget
            .try_reserve(Pool::OpenIndexes, bytes)
            .map_err(|e| ResolveError::TooBig(e.to_string()))?;
        let value: Value = serde_json::from_slice(doc.get(..len).unwrap_or_default())
            .map_err(|_| ResolveError::Failed("a stored object is unreadable".to_owned()))?;
        let loaded = match which {
            Which::Model => Loaded::Model(Arc::new(resolved(&value)?)),
            Which::Features => Loaded::Features(Arc::new(FeatureSet {
                features: features_of(value.pointer("/featureset/features"))?,
            })),
        };
        Ok((loaded, version, held, bytes))
    }
}

fn features_of(set: Option<&Value>) -> Result<Vec<Feature>, ResolveError> {
    let unreadable = || ResolveError::Failed("a stored feature set is unreadable".to_owned());
    let set = set.and_then(Value::as_array).ok_or_else(unreadable)?;
    // ALLOC: one entry a feature of the set, at most MAX_FEATURES.
    let mut features = Vec::with_capacity(set.len());
    for f in set {
        let name = f
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(unreadable)?;
        let template = match f.get("template") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => return Err(unreadable()),
        };
        let params = f
            .get("params")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        // ALLOC: one feature, bounded by the stored set.
        features.push(Feature {
            name: name.to_owned(),
            params,
            template,
        });
    }
    Ok(features)
}

fn resolved(doc: &Value) -> Result<Resolved, ResolveError> {
    let unreadable = || ResolveError::Failed("a stored model is unreadable".to_owned());
    let model = doc.pointer("/model/model").ok_or_else(unreadable)?;
    let kind = model
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(unreadable)?;
    let definition = model
        .get("definition")
        .and_then(Value::as_str)
        .ok_or_else(unreadable)?;
    let features = features_of(doc.pointer("/model/feature_set/features"))?;
    let names: Vec<String> = features.iter().map(|f| f.name.clone()).collect();
    let evaluator = Evaluator::parse(kind, definition, &names)
        .map_err(|e| ResolveError::Failed(format!("a stored model no longer parses: {e}")))?;
    Ok(Resolved {
        evaluator,
        features,
    })
}

impl Models for StoreModels {
    fn resolve(&self, store: Option<&str>, name: &str) -> Result<Arc<Resolved>, ResolveError> {
        match self.cached(Which::Model, store, name)? {
            Loaded::Model(m) => Ok(m),
            Loaded::Features(_) => Err(ResolveError::Failed(
                "a cache entry of the wrong kind".into(),
            )),
        }
    }

    fn feature_set(
        &self,
        store: Option<&str>,
        name: &str,
    ) -> Result<Arc<FeatureSet>, ResolveError> {
        match self.cached(Which::Features, store, name)? {
            Loaded::Features(f) => Ok(f),
            Loaded::Model(_) => Err(ResolveError::Failed(
                "a cache entry of the wrong kind".into(),
            )),
        }
    }

    fn max_window(&self) -> usize {
        self.cache.max_window
    }
}

impl StoreModels {
    /// The object through the cache: held while fresh, its set's marker read
    /// once it is older than the TTL, read again only when the set moved.
    fn cached(
        &self,
        which: Which,
        store: Option<&str>,
        name: &str,
    ) -> Result<Loaded, ResolveError> {
        let out = self.lookup(which, store, name);
        let mut state = self.lock();
        state.requests = state.requests.saturating_add(1);
        if out.is_err() {
            state.errors = state.errors.saturating_add(1);
        }
        out
    }

    fn lookup(
        &self,
        which: Which,
        store: Option<&str>,
        name: &str,
    ) -> Result<Loaded, ResolveError> {
        let store = store.unwrap_or(DEFAULT);
        // ALLOC: one key a resolve.
        let key = (which, store.to_owned(), name.to_owned());
        let stale = {
            let mut state = self.lock();
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            match state.entries.get_mut(&key) {
                Some(e) if e.checked.elapsed() < self.cache.ttl => {
                    e.used = tick;
                    let found = e.resolved.clone();
                    state.hits[which.slot()] = state.hits[which.slot()].saturating_add(1);
                    return Ok(found);
                }
                Some(e) => Some(e.version),
                None => None,
            }
        };
        if let Some(version) = stale {
            // One marker read: has the set moved since this was read?
            let set = self.set(store, which)?;
            let _read = self
                .budget
                .try_reserve(Pool::OpenIndexes, self.cache.buffer_bytes)
                .map_err(|e| ResolveError::TooBig(e.to_string()))?;
            // ALLOC: one marker buffer, reserved above.
            let mut marker = vec![0u8; self.cache.buffer_bytes];
            let now = set
                .version(&mut marker)
                .map_err(|e| ResolveError::Failed(e.to_string()))?;
            if now == version {
                let mut state = self.lock();
                if let Some(e) = state.entries.get_mut(&key) {
                    e.checked = Instant::now();
                    let found = e.resolved.clone();
                    state.hits[which.slot()] = state.hits[which.slot()].saturating_add(1);
                    return Ok(found);
                }
            }
        }
        let (resolved, version, held, bytes) = self.load(which, store, name)?;
        let mut state = self.lock();
        state.misses[which.slot()] = state.misses[which.slot()].saturating_add(1);
        if let Some(old) = state.entries.remove(&key) {
            state.held = state.held.saturating_sub(old.bytes);
        }
        // Make room: drop the least recently used until this fits.
        while state.held.saturating_add(bytes) > self.cache.bytes {
            let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(gone) = state.entries.remove(&oldest) {
                state.held = state.held.saturating_sub(gone.bytes);
                state.evictions[oldest.0.slot()] =
                    state.evictions[oldest.0.slot()].saturating_add(1);
            }
        }
        state.tick = state.tick.saturating_add(1);
        let tick = state.tick;
        state.held = state.held.saturating_add(bytes);
        state.entries.insert(
            key,
            Entry {
                resolved: resolved.clone(),
                version,
                checked: Instant::now(),
                used: tick,
                _held: held,
                bytes,
            },
        );
        Ok(resolved)
    }

    /// How many models the cache holds, and their bytes.
    pub fn held(&self) -> (usize, usize) {
        let state = self.lock();
        (state.entries.len(), state.held)
    }

    /// What the node's cache has counted and holds.
    pub fn stats(&self) -> NodeStats {
        let state = self.lock();
        let of = |w: Which| {
            let mut c = CacheCounts {
                hits: state.hits[w.slot()],
                misses: state.misses[w.slot()],
                evictions: state.evictions[w.slot()],
                ..CacheCounts::default()
            };
            for (k, e) in &state.entries {
                if k.0 == w {
                    c.entries = c.entries.saturating_add(1);
                    c.bytes = c.bytes.saturating_add(e.bytes);
                }
            }
            c
        };
        NodeStats {
            model: of(Which::Model),
            featureset: of(Which::Features),
            requests: state.requests,
            errors: state.errors,
        }
    }

    /// Forgets what is cached of `store` (the default store when `None`),
    /// and returns how many entries went.
    pub fn clear_store(&self, store: &str) -> usize {
        let mut state = self.lock();
        let gone: Vec<_> = state
            .entries
            .keys()
            .filter(|k| k.1 == store)
            .cloned()
            .collect();
        for k in &gone {
            if let Some(e) = state.entries.remove(k) {
                state.held = state.held.saturating_sub(e.bytes);
            }
        }
        gone.len()
    }

    /// Forgets every cached model.
    pub fn clear(&self) {
        let mut state = self.lock();
        state.entries.clear();
        state.held = 0;
    }
}
