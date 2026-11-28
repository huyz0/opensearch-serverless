//! What a node holds for local models: the runtime it may load and the memory each
//! deployed model is charged, reserved before anything loads and returned on undeploy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_rest::RestError;
use serde_json::{Map, Value};

use super::{MlService, bad, refuse};

/// A model's charge is its file's bytes times this, for the tensors and the runner's own
/// state: provisional until measured against the runtime.
const OVERHEAD: usize = 2;
/// The default for the most local models one node holds.
pub(super) const MAX_MODELS: usize = 8;

/// The node's local-model budget and what is charged against it.
pub struct Local {
    budget: MemoryBudget,
    runtime: Option<PathBuf>,
    max_models: usize,
    held: Mutex<HashMap<String, Reservation>>,
}

impl Local {
    /// Local models charged to `budget`'s models pool, loaded through the runtime
    /// library at `runtime`; with no runtime, local models are refused.
    #[must_use]
    pub fn new(budget: &MemoryBudget, runtime: Option<PathBuf>) -> Self {
        Self {
            budget: budget.clone(),
            runtime,
            max_models: MAX_MODELS,
            held: Mutex::new(HashMap::new()),
        }
    }

    /// The most models this node holds at once.
    #[must_use]
    pub fn with_max_models(mut self, max: usize) -> Self {
        self.max_models = max;
        self
    }

    /// Bytes charged for a model whose file is `file_bytes` long.
    fn charge_of(file_bytes: u64) -> usize {
        usize::try_from(file_bytes)
            .unwrap_or(usize::MAX)
            .saturating_mul(OVERHEAD)
    }
}

impl MlService {
    /// Refuses local models on a node with no runtime for them.
    pub(super) fn require_runtime(&self) -> Result<(), RestError> {
        match &self.local {
            Some(l) if l.runtime.is_some() => Ok(()),
            _ => Err(bad(
                "local models need a model runtime, and this node has none: set OSS_ML_ONNX_RUNTIME_PATH",
            )),
        }
    }

    /// Reserves the memory a local model needs; 429 when the pool or the count is full.
    pub(super) fn charge_model(
        &self,
        id: &str,
        model: &Map<String, Value>,
    ) -> Result<(), RestError> {
        self.require_runtime()?;
        let Some(local) = &self.local else {
            return Ok(());
        };
        let mut held = local.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.contains_key(id) {
            return Ok(());
        }
        if held.len() >= local.max_models {
            return Err(refuse(
                429,
                "circuit_breaking_exception",
                "This node holds the most models it may; undeploy one first",
            ));
        }
        let bytes = Local::charge_of(
            model
                .get("model_content_size_in_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        let reservation = local.budget.try_reserve(Pool::Models, bytes).map_err(|_| {
            refuse(
                429,
                "circuit_breaking_exception",
                "Memory Circuit Breaker is open, please check your resources!",
            )
        })?;
        // ALLOC: one entry a deployed local model, at most max_models.
        held.insert(id.to_owned(), reservation);
        Ok(())
    }

    /// Returns what a model was charged.
    pub(super) fn release_model(&self, id: &str) {
        if let Some(local) = &self.local {
            local
                .held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
        }
    }
}
