//! Reading the WASM modules a policy names from the store at start: one GET
//! each, checked against the hash the policy states
//! (docs/specs/plugin-system AC-4, AC-12).

use std::collections::HashMap;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_ext::manifest::{Manifest, ManifestError};
use oss_ext::native::NativeInstance;
use oss_ext::{CallError, DescriptorError, Status, Tier};
use oss_mem::MemoryBudget;

use super::policy::{Policy, PolicyTier};
use super::wasm::WasmPlugin;
use crate::extension_store::{self as store, StoreError};

/// A module that could not be loaded: it holds the reason the registry shows
/// for the plugin, and serves nothing.
struct Unloaded(&'static str);

impl NativeInstance for Unloaded {
    fn manifest(&self) -> Vec<u8> {
        Vec::new()
    }

    fn tier(&self) -> Tier {
        Tier::Wasm
    }

    fn read_manifest(&self) -> Result<Manifest, ManifestError> {
        Err(DescriptorError::new("module", self.0).into())
    }

    fn call(&self, _: &str, _: &[u8], _: usize) -> Result<Vec<u8>, CallError> {
        Err(CallError::from_status(Status::Unsupported, b""))
    }
}

fn reason(e: &StoreError) -> &'static str {
    match e {
        StoreError::Blob(_) => "could not be read from the store",
        StoreError::Mismatch => "does not hash to the policy's sha256",
        StoreError::Load(oss_ext_wasm::LoadError::Busy) => {
            "no extensions memory to load it (OSS_EXTENSIONS_PERCENT is 0)"
        }
        StoreError::Load(_) => "was refused by the extension host",
    }
}

/// Reads every module the policy names by hash, and answers the plugins to
/// register under each entry's name. A module that cannot be read, checked or
/// compiled is registered as one that says why, so the registry refuses that
/// plugin alone.
pub fn load_modules(
    policy: &Policy,
    store: &dyn BlobStore,
    base: &str,
    budget: &MemoryBudget,
) -> HashMap<String, Arc<dyn NativeInstance>> {
    let host = crate::extension_host::host();
    let mut out: HashMap<String, Arc<dyn NativeInstance>> = HashMap::new();
    for entry in policy.entries() {
        let (PolicyTier::Wasm, Some(hash)) = (entry.tier, entry.sha256.as_deref()) else {
            continue;
        };
        let instance: Arc<dyn NativeInstance> = match store::load(store, base, hash, &host, budget)
        {
            Ok(module) => Arc::new(WasmPlugin::new(module)),
            Err(e) => Arc::new(Unloaded(reason(&e))),
        };
        out.insert(entry.name.clone(), instance);
    }
    out
}
