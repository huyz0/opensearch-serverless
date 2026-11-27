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
use oss_ext::analyzer::{Analyze, ITEM_DEADLINES, Token};
use oss_ext::pin::{Pin, ResolvedLimits, read_pins};
use oss_ext_wasm::{Host, LoadError, Module, Recovery, Replay, Run};
use oss_mem::MemoryBudget;
use oss_writer::mapping::json::{Json, parse};

use crate::extension_store::{self as store, StoreError};

/// How long a replay waits for its reservation before the open is refused as
/// retryable.
const REPLAY_WAIT: Duration = Duration::from_secs(60);

/// How long a tail overlay build waits for a call's reservation before the
/// search answers a retryable 503.
const OVERLAY_WAIT: Duration = Duration::from_secs(5);

/// How long a module whose open was refused is not read again.
const REFUSED_FOR_MILLIS: u64 = 30_000;

/// The most refused modules remembered; a refusal past it is not kept.
const MAX_REFUSED: usize = 1024;

/// One pin's analyzer: a loaded module, the limits recorded for it, and the
/// tenant its calls are counted to.
pub struct ModuleTokens {
    module: Arc<Module>,
    limits: ResolvedLimits,
    factor: u64,
    tenant: String,
    module_name: String,
    replaying: Arc<Replays>,
    /// Set for the tail overlay's copy: calls skip the gate and the tenant's
    /// share and wait this long for a reservation.
    recovery: Option<Duration>,
}

impl Analyze for ModuleTokens {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let replaying = self.replaying.on.load(Ordering::Acquire);
        let run = if let Some(wait) = self.recovery {
            Run::Recovery(Recovery {
                limits: self.limits,
                wait,
            })
        } else if replaying {
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

    fn item_budget(&self) -> Option<Duration> {
        // A replay and a recovery run under the record, whatever the clock says.
        if self.recovery.is_some() || self.replaying.on.load(Ordering::Acquire) {
            return None;
        }
        Some(Duration::from_millis(self.limits.deadline_ms).saturating_mul(ITEM_DEADLINES))
    }
}

impl ModuleTokens {
    /// This analyzer as the tail overlay's build calls it: a recovery of
    /// state, so not refused by the gate or the tenant's share.
    fn for_overlay(&self) -> Arc<dyn Analyze> {
        Arc::new(Self {
            module: Arc::clone(&self.module),
            limits: self.limits,
            factor: self.factor,
            // ALLOC: two names a pin, once an overlay build.
            tenant: self.tenant.clone(),
            module_name: self.module_name.clone(),
            replaying: Arc::clone(&self.replaying),
            recovery: Some(OVERLAY_WAIT),
        })
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
    /// Modules whose open was refused, by hash: when the refusal ends and
    /// why, so a module that cannot run costs one read in 30 seconds, not
    /// one a shard open.
    refused: Mutex<HashMap<String, (u64, String)>>,
    /// The time in milliseconds, from any fixed start.
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
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
            refused: Mutex::new(HashMap::new()),
            clock: Arc::new(|| {
                static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
                u64::try_from(
                    START
                        .get_or_init(std::time::Instant::now)
                        .elapsed()
                        .as_millis(),
                )
                .unwrap_or(u64::MAX)
            }),
        }
    }

    /// This source reading time from `clock` (milliseconds from any fixed
    /// start).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
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
        let pins = read_pins(&flat).map_err(|e| {
            let why = unavailable(e.to_string());
            crate::extension_refusals::record(&why);
            why
        })?;
        self.open_pins(pins, tenant)
    }

    /// As [`ModuleSource::open`], for pins already read from the settings.
    ///
    /// # Errors
    ///
    /// As [`ModuleSource::open`], minus the pins not reading.
    pub fn open_pins(
        &self,
        pins: Vec<(Pin, ResolvedLimits)>,
        tenant: &str,
    ) -> Result<ModuleSet, String> {
        self.load_pins(pins, tenant)
            .inspect_err(|why| crate::extension_refusals::record(why))
    }

    fn load_pins(
        &self,
        pins: Vec<(Pin, ResolvedLimits)>,
        tenant: &str,
    ) -> Result<ModuleSet, String> {
        let unavailable = |why: String| format!("extension_unavailable: {why}");
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
                    recovery: None,
                }),
            ));
        }
        Ok(ModuleSet {
            analyzers,
            replaying,
        })
    }

    /// Why `hash` was refused within the last 30 seconds, if it was.
    fn refused_for(&self, hash: &str) -> Option<String> {
        let now = (self.clock)();
        let mut refused = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
        match refused.get(hash) {
            Some((until, why)) if *until > now => Some(why.clone()), // ALLOC: error path.
            Some(_) => {
                refused.remove(hash);
                None
            }
            None => None,
        }
    }

    /// Remembers that `hash` was refused, unless that was for capacity,
    /// which passes.
    fn refuse(&self, hash: &str, why: &str) {
        if why.contains(crate::search_modules::NO_CAPACITY) {
            return;
        }
        let now = (self.clock)();
        let mut refused = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
        if refused.len() >= MAX_REFUSED {
            refused.retain(|_, (until, _)| *until > now);
        }
        if refused.len() < MAX_REFUSED {
            // ALLOC: one entry a refused module, at most MAX_REFUSED.
            refused.insert(
                hash.to_owned(),
                (now.saturating_add(REFUSED_FOR_MILLIS), why.to_owned()),
            );
        }
    }

    fn load(&self, pin: &Pin) -> Result<Module, String> {
        if let Some(why) = self.refused_for(&pin.module) {
            return Err(why);
        }
        self.load_unchecked(pin)
            .inspect_err(|why| self.refuse(&pin.module, why))
    }

    fn load_unchecked(&self, pin: &Pin) -> Result<Module, String> {
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
                format!(
                    "{} module [{}]; retry",
                    crate::search_modules::NO_CAPACITY,
                    pin.module
                )
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

impl oss_reader::pinned::PinnedTokens for ModuleSet {
    fn tokens(&self, analyzer: &str, text: &str) -> Result<Vec<Token>, String> {
        let Some((_, tokens)) = self.analyzers.iter().find(|(name, _)| name == analyzer) else {
            // ALLOC: error path; one message.
            return Err(format!(
                "extension_unavailable: no analyzer [{analyzer}] is loaded"
            ));
        };
        // ALLOC: error path; one message.
        tokens.analyze(text).map_err(|e| {
            // A call the host had no capacity for is retried, not refused.
            let kind = if e.kind() == oss_ext::FailureKind::Busy {
                "extension_busy"
            } else {
                "extension_unavailable"
            };
            format!("{kind}: analyzer [{analyzer}]: {}", e.detail())
        })
    }

    fn modules(&self) -> Vec<(String, Arc<dyn Analyze>)> {
        // ALLOC: one entry a pin, at most four.
        self.analyzers
            .iter()
            .map(|(name, t)| (name.clone(), t.for_overlay()))
            .collect()
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

impl crate::search_modules::ModuleOpener for ModuleSource {
    fn open(
        &self,
        index: &str,
        pins: Vec<(Pin, ResolvedLimits)>,
    ) -> Result<Arc<dyn oss_reader::pinned::PinnedTokens>, String> {
        let set = self.open_pins(pins, index)?;
        Ok(Arc::new(set))
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
