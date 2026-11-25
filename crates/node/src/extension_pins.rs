//! The pin policy over the module store
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)): a
//! pin is accepted when its module is in the store, can be an analyzer and
//! answers a probe, and the limits it will run under are resolved then.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore};
use oss_ext::pin::{Pin, ResolvedLimits};
use oss_ext_wasm::{Host, LoadError};
use oss_mem::MemoryBudget;
use oss_types::settings::Live;

use crate::cluster_live::ClusterFeed;
use crate::extension_setting::PINS_ALLOWED;
use crate::extension_store::{self as store, StoreError};
use crate::pin_policy::PinPolicy;

/// The text a module is asked to analyse when it is pinned.
const PROBE: &str = "probe";
/// The tenant a probe is charged to.
const PROBE_TENANT: &str = "_pin";

/// Who may pin: the node says, from the request's user.
pub type Admin = Arc<dyn Fn(Option<&str>) -> bool + Send + Sync>;

/// A [`PinPolicy`] that reads modules from a store.
pub struct ModulePins {
    store: Arc<dyn BlobStore>,
    base: String,
    host: Host,
    budget: MemoryBudget,
    admin: Admin,
    gate: Option<ClusterFeed>,
}

impl ModulePins {
    /// A policy over `store` under `base`, compiling and probing in `host`
    /// with memory charged to `budget`'s `extensions` pool; `admin` says who
    /// holds `all_access`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        host: Host,
        budget: MemoryBudget,
        admin: Admin,
    ) -> Self {
        Self {
            store,
            // ALLOC: one prefix a node.
            base: base.to_owned(),
            host,
            budget,
            admin,
            gate: None,
        }
    }

    /// Lets a pin through only while the cluster register says
    /// `serverless.extensions.pins.allowed` is `true`, read once for each
    /// create that carries a pin and never for one that does not. Without
    /// this the policy allows, for a caller that decides elsewhere.
    ///
    /// # Errors
    ///
    /// A base that is not empty and does not end in `/`.
    pub fn gated_by_the_cluster_setting(mut self) -> Result<Self, oss_meta::MetaError> {
        self.gate = Some(ClusterFeed::new(
            Arc::clone(&self.store),
            &self.base,
            Arc::new(Live::new()),
        )?);
        Ok(self)
    }
}

impl PinPolicy for ModulePins {
    fn authorised(&self, principal: Option<&str>) -> bool {
        (self.admin)(principal)
    }

    fn allowed(&self) -> Result<(), String> {
        let Some(gate) = &self.gate else {
            return Ok(());
        };
        gate.read()
            .map_err(|e| format!("the cluster settings could not be read: {e}"))?;
        if PINS_ALLOWED.get(&*gate.live().current()).unwrap_or(false) {
            Ok(())
        } else {
            Err("pinned analyzers are not enabled on this cluster \
                 (serverless.extensions.pins.allowed)"
                .to_owned())
        }
    }

    fn check(&self, pin: &Pin) -> Result<ResolvedLimits, String> {
        let module = store::load(
            self.store.as_ref(),
            &self.base,
            &pin.module,
            &self.host,
            &self.budget,
        )
        .map_err(|e| match e {
            StoreError::Blob(BlobError::NotFound) => {
                // ALLOC: error path; one message.
                format!("module [{}] is not in the store", pin.module)
            }
            StoreError::Blob(e) => e.to_string(),
            StoreError::Mismatch => "the stored module does not match its hash".to_owned(),
            StoreError::Load(LoadError::Busy) => {
                "no capacity to compile the module; retry".to_owned()
            }
            StoreError::Load(e) => e.to_string(),
        })?;
        module
            .check_analyzer_pin(pin.version)
            .map_err(|e| e.to_string())?;
        module
            .analyze(PROBE_TENANT, &[PROBE])
            .map_err(|e| format!("the module failed its probe: {}", e.detail()))?;
        Ok(module.resolved_limits())
    }
}
