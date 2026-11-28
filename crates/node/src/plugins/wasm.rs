//! A WASM module as a plugin (docs/specs/plugin-system AC-4, AC-8): the
//! module's descriptor is its manifest, and each call is one bounded call
//! into a fresh instance of the extension host.

use oss_ext::CallError;
use oss_ext::Tier;
use oss_ext::manifest::{Manifest, ManifestError};
use oss_ext::native::NativeInstance;
use oss_ext_wasm::Module;

/// A loaded module that serves the points its descriptor declares.
pub struct WasmPlugin {
    module: Module,
}

impl WasmPlugin {
    /// Serves calls from `module`, which the extension host loaded.
    pub fn new(module: Module) -> Self {
        Self { module }
    }
}

impl NativeInstance for WasmPlugin {
    /// A module delivers its manifest through its descriptor
    /// ([`NativeInstance::read_manifest`]), so there are no manifest bytes.
    fn manifest(&self) -> Vec<u8> {
        Vec::new()
    }

    fn tier(&self) -> Tier {
        Tier::Wasm
    }

    fn read_manifest(&self) -> Result<Manifest, ManifestError> {
        Manifest::from_descriptor(self.module.descriptor())
    }

    fn call(&self, point: &str, input: &[u8], _max_out: usize) -> Result<Vec<u8>, CallError> {
        let index = self
            .module
            .descriptor()
            .points()
            .iter()
            .position(|p| p.id() == point)
            .ok_or_else(|| CallError::from_status(oss_ext::Status::Unsupported, b""))?;
        // The calling tenant is not named here: the plugin's guard has already
        // held the per-tenant place, and the module's own gate bounds the rest.
        self.module.call("", index, input)
    }
}
