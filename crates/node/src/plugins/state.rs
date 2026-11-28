//! A plugin's tenant-scoped state: the `state.get`, `state.put` and
//! `state.delete` host operations over a document set under
//! `plugins/<id>/<tenant>/` (ADR 0080, section 5).
//!
//! A tenant sees only its own set: the tenant is the host's, from the
//! authenticated request, and becomes a path segment only as a hash. Nothing
//! lists. Every other operation goes to the backend behind this one.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_ext::host::{Backend, HostError, Who};
use oss_ext::state::{decode_key, decode_put, encode_found};
use oss_format::{DocKind, MAX_DOC_SET_MARKER_BYTES, MAX_PLUGIN_STATE_BYTES};
use oss_mem::{MemoryBudget, Pool};
use oss_meta::{DocSet, MetaError};
use sha2::{Digest, Sha256};

/// Serves the `state.*` operations and hands the rest to `inner`.
pub struct StateBackend {
    store: Arc<dyn BlobStore>,
    base: String,
    budget: MemoryBudget,
    inner: Arc<dyn Backend>,
}

impl StateBackend {
    /// State under `base` of `store`, working memory from `budget`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        budget: &MemoryBudget,
        inner: Arc<dyn Backend>,
    ) -> Self {
        Self {
            store,
            // ALLOC: one copy of the base, once a node.
            base: base.to_owned(),
            budget: budget.clone(),
            inner,
        }
    }

    /// The set of `who`: the plugin's id and the tenant's hash are path
    /// segments, so no plugin or tenant can name another's.
    fn set(&self, who: &Who<'_>) -> Result<DocSet, HostError> {
        let tenant = if who.tenant.is_empty() {
            "_node".to_owned()
        } else {
            // ALLOC: one 64-character segment a call.
            Sha256::digest(who.tenant.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let base = format!("{}plugins/{}/{tenant}/", self.base, who.plugin);
        DocSet::new(Arc::clone(&self.store), &base, DocKind::PluginState)
            .map_err(|_| HostError::Failed)
    }

    fn run(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError> {
        let want = MAX_DOC_SET_MARKER_BYTES.saturating_add(MAX_PLUGIN_STATE_BYTES);
        let _held = self
            .budget
            .try_reserve(Pool::RequestBodies, want)
            .map_err(|_| HostError::Unavailable)?;
        // ALLOC: one marker buffer and one value buffer a call, reserved above.
        let mut marker = vec![0_u8; MAX_DOC_SET_MARKER_BYTES];
        let set = self.set(who)?;
        match op {
            "state.get" => {
                let key = decode_key(input).map_err(|_| HostError::Invalid)?;
                // ALLOC: one value buffer a call, reserved above.
                let mut body = vec![0_u8; MAX_PLUGIN_STATE_BYTES];
                let n = set.get(&key, &mut marker, &mut body).map_err(refused)?;
                Ok(encode_found(n.map(|n| body.get(..n).unwrap_or_default())))
            }
            "state.put" => {
                let put = decode_put(input).map_err(|_| HostError::Invalid)?;
                match set.put(&put.key, &put.value, put.if_absent, &mut marker) {
                    Ok(()) => Ok(vec![1]),
                    Err(MetaError::AlreadyExists) => Ok(vec![0]),
                    Err(e) => Err(refused(e)),
                }
            }
            "state.delete" => {
                let key = decode_key(input).map_err(|_| HostError::Invalid)?;
                let gone = set.delete(&key, &mut marker).map_err(refused)?;
                Ok(vec![u8::from(gone)])
            }
            _ => Err(HostError::Unknown),
        }
    }
}

/// What a set's failure is to the plugin that asked.
fn refused(e: MetaError) -> HostError {
    match e {
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => HostError::Invalid,
        MetaError::TooManyDocs => HostError::TooLarge,
        MetaError::Contended => HostError::Unavailable,
        _ => HostError::Failed,
    }
}

impl Backend for StateBackend {
    fn call(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError> {
        if op.starts_with("state.") {
            self.run(who, op, input)
        } else {
            self.inner.call(who, op, input)
        }
    }
}
