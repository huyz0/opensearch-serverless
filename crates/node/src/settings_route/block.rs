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
        self.set_one(index, &format!(r#"{{"index.blocks.write":{on}}}"#))
    }

    /// Sets `index.number_of_replicas` of `index` to `n`, as a request does:
    /// the route refuses any count but 0, and this says so the same way.
    ///
    /// # Errors
    ///
    /// As [`SettingsService::set_write_block`].
    pub fn set_replicas(&self, index: &str, n: u32) -> Result<(), String> {
        self.set_one(index, &format!(r#"{{"index.number_of_replicas":{n}}}"#))
    }

    /// One settings body for one index, through the route's merge and swap.
    fn set_one(&self, index: &str, body: &str) -> Result<(), String> {
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
