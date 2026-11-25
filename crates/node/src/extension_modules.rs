//! The modules an index's pins name, loaded when a shard of it opens
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)):
//! one read for each distinct module, then a call under the limits the
//! descriptor recorded, or stretched for a replay.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use oss_blob::BlobStore;
use oss_ext::CallError;
use oss_ext::analyzer::{Analyze, Token};
use oss_ext::pin::{Pin, ResolvedLimits, read_pins};
use oss_ext_wasm::{Host, LoadError, Module, Replay, Run};
use oss_mem::MemoryBudget;
use oss_writer::mapping::json::{Json, parse};

use crate::extension_store::{self as store, StoreError};

/// How long a replay waits for its reservation before the open is refused as
/// retryable.
const REPLAY_WAIT: Duration = Duration::from_secs(60);

/// One pin's analyzer: a loaded module, the limits recorded for it, and the
/// tenant its calls are counted to.
pub struct ModuleTokens {
    module: Arc<Module>,
    limits: ResolvedLimits,
    factor: u64,
    tenant: String,
    module_name: String,
    replaying: Arc<Replays>,
}

impl Analyze for ModuleTokens {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let replaying = self.replaying.on.load(Ordering::Acquire);
        let run = if replaying {
            Run::Replay(Replay {
                limits: self.limits,
                factor: self.factor,
                wait: REPLAY_WAIT,
            })
        } else {
            Run::Recorded(self.limits)
        };
        let mut all = self
            .module
            .analyze_as(&self.tenant, &[text], run)
            .inspect_err(|e| {
                if replaying {
                    self.replaying.failed(&self.module_name, e);
                }
            })?;
        all.pop()
            .ok_or_else(|| CallError::malformed_because("no tokens for the text"))
    }
}

/// Whether a set's calls are a replay's, and the replay call that failed.
#[derive(Default)]
struct Replays {
    on: AtomicBool,
    failures: AtomicU64,
    last: Mutex<Option<String>>,
}

impl Replays {
    fn failed(&self, module: &str, why: &CallError) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        // ALLOC: one message, on a failed replay call.
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(format!("module [{module}]: {why:?}"));
    }
}

/// The analyzers of one index, by the names its mapping gives them.
pub struct ModuleSet {
    analyzers: Vec<(String, Arc<ModuleTokens>)>,
    replaying: Arc<Replays>,
}

impl ModuleSet {
    /// Whether the index pins nothing.
    pub fn is_empty(&self) -> bool {
        self.analyzers.is_empty()
    }

    /// How many pins there are.
    pub fn len(&self) -> usize {
        self.analyzers.len()
    }

    /// The analyzers and the names that select them.
    pub fn analyzers(&self) -> impl Iterator<Item = (&str, Arc<dyn Analyze>)> {
        self.analyzers
            .iter()
            .map(|(name, a)| (name.as_str(), Arc::clone(a) as Arc<dyn Analyze>))
    }

    /// Runs every call from now under the stretched limits and the
    /// admission of a replay when `on`, and as a live write again when not.
    pub fn replaying(&self, on: bool) {
        self.replaying.on.store(on, Ordering::Release);
    }

    /// The replay call that failed, if one did since it was last taken:
    /// the module and why. The shard that was replaying does not open.
    pub fn replay_failure(&self) -> Option<String> {
        self.replaying
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// How many replay calls have failed over the life of the set.
    pub fn replay_failures(&self) -> u64 {
        self.replaying.failures.load(Ordering::Relaxed)
    }
}

/// Where a node reads the modules of its indexes from.
pub struct ModuleSource {
    store: Arc<dyn BlobStore>,
    base: String,
    host: Host,
    budget: MemoryBudget,
    factor: u64,
}

impl ModuleSource {
    /// A source over `store` under `base`, compiling in `host` with memory
    /// charged to `budget`'s `extensions` pool; a replay runs under its
    /// record times `factor` (`OSS_EXTENSIONS_REPLAY_FUEL_FACTOR`).
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        host: Host,
        budget: MemoryBudget,
        factor: u64,
    ) -> Self {
        Self {
            store,
            // ALLOC: one prefix a node.
            base: base.to_owned(),
            host,
            budget,
            factor,
        }
    }

    /// The analyzers an index's descriptor `settings` pin, each distinct
    /// module read once and charged to the extensions pool for as long as
    /// the set lives; an index with no pin reads nothing.
    ///
    /// # Errors
    ///
    /// The reason an open is refused, which starts with
    /// `extension_unavailable`: the pins do not read, a module is missing or
    /// does not match its hash, cannot be an analyzer, or the pool cannot
    /// hold it. A node that cannot run a pin never falls back to the
    /// built-in analyzer.
    pub fn open(&self, settings: &[u8], tenant: &str) -> Result<ModuleSet, String> {
        let unavailable = |why: String| format!("extension_unavailable: {why}");
        let members = match parse(settings) {
            Ok(Json::Obj(members)) => members,
            _ => Vec::new(),
        };
        // ALLOC: the settings of one descriptor, bounded by its size.
        let flat: Vec<(String, Option<String>)> = members
            .iter()
            .map(|(k, v)| (k.clone(), text_of(v)))
            .collect();
        let pins = read_pins(&flat).map_err(|e| unavailable(e.to_string()))?;
        let replaying = Arc::new(Replays::default());
        let mut loaded: HashMap<String, Arc<Module>> = HashMap::new();
        let mut analyzers = Vec::new();
        for (pin, limits) in pins {
            let module = match loaded.get(&pin.module) {
                Some(m) => Arc::clone(m),
                None => {
                    let m = Arc::new(self.load(&pin).map_err(unavailable)?);
                    // ALLOC: one entry a distinct module, at most four.
                    loaded.insert(pin.module.clone(), Arc::clone(&m));
                    m
                }
            };
            // ALLOC: one entry a pin, at most four.
            analyzers.push((
                pin.name,
                Arc::new(ModuleTokens {
                    module,
                    limits,
                    factor: self.factor,
                    // ALLOC: one name a pin.
                    tenant: tenant.to_owned(),
                    // ALLOC: one name a pin.
                    module_name: pin.module.clone(),
                    replaying: Arc::clone(&replaying),
                }),
            ));
        }
        Ok(ModuleSet {
            analyzers,
            replaying,
        })
    }

    fn load(&self, pin: &Pin) -> Result<Module, String> {
        let module = store::load(
            self.store.as_ref(),
            &self.base,
            &pin.module,
            &self.host,
            &self.budget,
        )
        .map_err(|e| match e {
            StoreError::Blob(oss_blob::BlobError::NotFound) => {
                // ALLOC: error path; one message.
                format!("module [{}] is not in the store", pin.module)
            }
            StoreError::Blob(e) => e.to_string(),
            StoreError::Mismatch => format!("module [{}] does not match its hash", pin.module),
            StoreError::Load(LoadError::Busy) => {
                format!("no capacity to compile module [{}]; retry", pin.module)
            }
            StoreError::Load(e) => e.to_string(),
        })?;
        module
            .check_analyzer_pin(pin.version)
            .map_err(|e| format!("module [{}]: {e}", pin.module))?;
        Ok(module)
    }
}

/// A setting's value as the text it was written as.
fn text_of(v: &Json) -> Option<String> {
    match v {
        Json::Str(s) => Some(s.clone()),
        _ => None,
    }
}

impl crate::writer::PinnedModules for ModuleSet {
    fn analyzers(&self) -> Vec<(String, Arc<dyn Analyze>)> {
        // ALLOC: one entry a pin, at most four.
        self.analyzers()
            .map(|(name, a)| (name.to_owned(), a))
            .collect()
    }

    fn replaying(&self, on: bool) {
        ModuleSet::replaying(self, on);
    }

    fn replay_failure(&self) -> Option<String> {
        ModuleSet::replay_failure(self)
    }
}

impl crate::writer::ModuleLoader for ModuleSource {
    fn load(
        &self,
        settings: &[u8],
        tenant: &str,
    ) -> Result<Option<Arc<dyn crate::writer::PinnedModules>>, String> {
        let set = self.open(settings, tenant)?;
        Ok((!set.is_empty()).then(|| Arc::new(set) as Arc<dyn crate::writer::PinnedModules>))
    }
}
