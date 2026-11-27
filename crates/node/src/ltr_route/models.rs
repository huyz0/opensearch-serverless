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
use oss_reader::ltr::models::{Feature, Models, ResolveError, Resolved};
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

struct Entry {
    resolved: Arc<Resolved>,
    /// The models set's change count when this was read.
    version: i64,
    checked: Instant,
    used: u64,
    _held: Reservation,
    bytes: usize,
}

#[derive(Default)]
struct State {
    entries: HashMap<(String, String), Entry>,
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

    fn set(&self, store: &str) -> Result<DocSet, ResolveError> {
        // ALLOC: one key prefix, once a load.
        let base = format!("{}ltr/{store}/", self.base);
        DocSet::new_created(Arc::clone(&self.store), &base, DocKind::LtrModels)
            .map_err(|e| ResolveError::Failed(e.to_string()))
    }

    /// Reads and parses the model, charging what it will hold first.
    fn load(
        &self,
        store: &str,
        name: &str,
    ) -> Result<(Arc<Resolved>, i64, Reservation, usize), ResolveError> {
        let set = self.set(store)?;
        let missing = || {
            ResolveError::NotFound(format!(
                "model [{name}] does not exist in the store [{store}]"
            ))
        };
        let max = DocKind::LtrModels.max_doc_bytes();
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
            .map_err(|_| ResolveError::Failed("a stored model is unreadable".to_owned()))?;
        Ok((Arc::new(resolved(&value)?), version, held, bytes))
    }
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
    let set = doc
        .pointer("/model/feature_set/features")
        .and_then(Value::as_array)
        .ok_or_else(unreadable)?;
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
        let store = store.unwrap_or(DEFAULT);
        // ALLOC: one key a resolve.
        let key = (store.to_owned(), name.to_owned());
        let stale = {
            let mut state = self.lock();
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            match state.entries.get_mut(&key) {
                Some(e) if e.checked.elapsed() < self.cache.ttl => {
                    e.used = tick;
                    return Ok(Arc::clone(&e.resolved));
                }
                Some(e) => Some(e.version),
                None => None,
            }
        };
        if let Some(version) = stale {
            // One marker read: has the set moved since this was read?
            let set = self.set(store)?;
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
                    return Ok(Arc::clone(&e.resolved));
                }
            }
        }
        let (resolved, version, held, bytes) = self.load(store, name)?;
        let mut state = self.lock();
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
            }
        }
        state.tick = state.tick.saturating_add(1);
        let tick = state.tick;
        state.held = state.held.saturating_add(bytes);
        state.entries.insert(
            key,
            Entry {
                resolved: Arc::clone(&resolved),
                version,
                checked: Instant::now(),
                used: tick,
                _held: held,
                bytes,
            },
        );
        Ok(resolved)
    }

    fn max_window(&self) -> usize {
        self.cache.max_window
    }
}

impl StoreModels {
    /// How many models the cache holds, and their bytes.
    pub fn held(&self) -> (usize, usize) {
        let state = self.lock();
        (state.entries.len(), state.held)
    }

    /// Forgets every cached model.
    pub fn clear(&self) {
        let mut state = self.lock();
        state.entries.clear();
        state.held = 0;
    }
}
