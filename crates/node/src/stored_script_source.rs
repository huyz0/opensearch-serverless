//! Where a script named by `id` finds its source: the stored-script set
//! (`_scripts/{id}`), read through a snapshot the node keeps and checks once
//! a TTL, so a script used by every search costs a marker GET a TTL and not
//! a read a search or a shard.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_mem::{MemoryBudget, Pool};
use oss_meta::{DocSet, DocSetCache, DocSnapshot, MetaError};
use oss_types::stored_scripts::{ResolveError, Stored, StoredScripts};

/// The language and source of a stored `{"script":{"lang","source"}}`.
pub(crate) fn script(body: &[u8]) -> Option<(String, String)> {
    let Json::Obj(top) = parse_json(body).ok()? else {
        return None;
    };
    let [(_, Json::Obj(members))] = top.as_slice() else {
        return None;
    };
    let (mut lang, mut source) = (None, None);
    for (name, value) in members {
        match (name.as_str(), value) {
            ("lang", Json::Str(s)) => lang = Some(s.clone()),
            ("source", Json::Str(s)) => source = Some(s.clone()),
            _ => {}
        }
    }
    Some((lang?, source?))
}

/// The most bytes of script documents the snapshot keeps.
pub const MAX_SNAPSHOT_BYTES: usize = 8 << 20;

/// The clock the TTL counts on, in milliseconds.
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

#[derive(Default)]
struct State {
    snapshot: Option<Arc<DocSnapshot>>,
    /// When the marker was last read and the generation then: a change since
    /// makes the check stale however recent.
    checked: Option<(u64, u64)>,
}

/// The stored-script set and what the node keeps of it.
pub struct ScriptSource {
    docs: DocSet,
    cache: DocSetCache,
    budget: MemoryBudget,
    ttl_millis: u64,
    now: Clock,
    generation: AtomicU64,
    state: Mutex<State>,
}

impl ScriptSource {
    /// The set under `base` in `store`, checked every `ttl_millis`; a refresh
    /// reserves its two read buffers from `budget`'s request-body pool.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn open(
        store: &Arc<dyn BlobStore>,
        base: &str,
        budget: &MemoryBudget,
        ttl_millis: u64,
        now: Clock,
    ) -> Result<Arc<Self>, MetaError> {
        // ALLOC: one holder, once, at boot.
        Ok(Arc::new(Self {
            docs: DocSet::new(Arc::clone(store), base, DocKind::StoredScript)?,
            cache: DocSetCache::with_budget(MAX_SNAPSHOT_BYTES),
            budget: budget.clone(),
            ttl_millis,
            now,
            generation: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }))
    }

    /// A script was put or deleted here: the next resolve reads the set, and
    /// a refresh already under way does not count as a check.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn refresh(&self, state: &mut State, now: u64) -> Result<(), ResolveError> {
        let generation = self.generation.load(Ordering::SeqCst);
        let bytes = MAX_DOC_BYTES.saturating_mul(2);
        let _held = self
            .budget
            .try_reserve(Pool::RequestBodies, bytes)
            .map_err(|_| {
                ResolveError::TooBig(String::from(
                    "this node's memory cannot hold a read of the stored scripts now",
                ))
            })?;
        // ALLOC: two scratch buffers a refresh, MAX_DOC_BYTES each, reserved above.
        let mut marker = vec![0u8; MAX_DOC_BYTES];
        let mut doc = vec![0u8; MAX_DOC_BYTES];
        let snapshot = self
            .docs
            .snapshot(&self.cache, &mut marker, &mut doc)
            // ALLOC: error path; one message.
            .map_err(|e| ResolveError::Failed(e.to_string()))?;
        if snapshot.retained_bytes() > MAX_SNAPSHOT_BYTES {
            return Err(ResolveError::TooBig(format!(
                "the stored scripts are over the {MAX_SNAPSHOT_BYTES} byte limit this node keeps; delete some"
            )));
        }
        state.snapshot = Some(snapshot);
        // A change that landed during the read leaves the generation behind.
        state.checked = Some((now, generation));
        Ok(())
    }
}

impl StoredScripts for ScriptSource {
    fn resolve(&self, id: &str) -> Result<Stored, ResolveError> {
        // One refresh at a time: resolves that arrive during it wait for it.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = (self.now)();
        let fresh = state.snapshot.is_some()
            && state.checked.is_some_and(|(at, generation)| {
                generation == self.generation.load(Ordering::SeqCst)
                    && now.saturating_sub(at) < self.ttl_millis
            });
        if !fresh {
            self.refresh(&mut state, now)?;
        }
        let snapshot = state.snapshot.as_ref().ok_or(ResolveError::Missing)?;
        let body = snapshot.get(id).ok_or(ResolveError::Missing)?;
        let (lang, source) = script(body).ok_or_else(|| {
            // ALLOC: error path; one message.
            ResolveError::Failed(format!("the script [{id}] is not a stored script"))
        })?;
        Ok(Stored { lang, source })
    }
}

#[cfg(test)]
mod tests;
