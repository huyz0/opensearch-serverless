//! The write block as the index state management runner sets it: the same
//! merge and descriptor swap the settings route makes, without a request.

use super::{SettingsService, Target, parse};

impl SettingsService {
    /// Sets (`on`) or lifts the write block of `index`: `index.blocks.write`
    /// through the route's own merge and swap, so a bulk reads it as it reads
    /// one set by a request, and setting what is set writes nothing.
    ///
    /// # Errors
    ///
    /// The reason the block could not be changed: no such index, or the
    /// swap kept losing.
    pub fn set_write_block(&self, index: &str, on: bool) -> Result<(), String> {
        let body = format!(r#"{{"index.blocks.write":{on}}}"#);
        let requested =
            parse(Some(body.as_bytes()), &self.registry).map_err(|r| r.reason.to_string())?;
        // ALLOC: two buffers of buffer_bytes for the one swap, freed with it;
        // the runner takes one step at a time.
        let mut read = vec![0u8; self.buffer_bytes];
        let mut write = vec![0u8; self.buffer_bytes];
        let held = self
            .descriptors
            .resolve(index, &mut read)
            .map_err(|e| e.to_string())?;
        if !matches!(held.target, Target::Index(_)) {
            return Err(format!("no such index [{index}]"));
        }
        self.update_one(index, &requested, false, &mut read, &mut write)
            .map_err(|_| format!("the settings of [{index}] could not be written"))
    }
}
