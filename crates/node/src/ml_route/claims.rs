//! The name claims of model groups: one blob a name, made by conditional create.

use oss_blob::{BlobError, ByteRange, CreateOutcome, SliceSource};
use oss_rest::RestError;
use serde_json::Value;

use super::{CLAIM_BUFFER, MlService, blob_failed, hex};

impl MlService {
    pub(super) fn claim_key(&self, name: &str) -> String {
        // ALLOC: one key a claim.
        format!("{}ml/model_group_names/public/{}", self.base, hex(name))
    }

    /// Makes the claim on `name` for `id`; the id holding it when another does.
    ///
    /// A claim that names no group of that name, and has outlived
    /// `stale_millis`, is the leftover of a register that died between its
    /// claim and its write: it is released (only if it still holds the id read)
    /// and made again once.
    pub(super) fn claim(
        &self,
        name: &str,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<String>, RestError> {
        let key = self.claim_key(name);
        let mut buf = [0u8; CLAIM_BUFFER];
        for _ in 0..2 {
            if matches!(
                self.store
                    .create_if_absent(&key, &mut SliceSource::new(id.as_bytes()), &mut buf)
                    .map_err(|e| blob_failed(&e))?,
                CreateOutcome::Created
            ) {
                return Ok(None);
            }
            let held = self.holder(name)?;
            if !self.is_stale(&key, name, held.as_deref(), marker, doc)? {
                return Ok(Some(held.unwrap_or_default()));
            }
            self.release(name, held.as_deref());
        }
        let held = self.holder(name)?;
        Ok(Some(held.unwrap_or_default()))
    }

    /// Whether the claim at `key` is a leftover: old, and its holder is not a
    /// group of this name.
    fn is_stale(
        &self,
        key: &str,
        name: &str,
        held: Option<&str>,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<bool, RestError> {
        let old = self
            .store
            .age_millis(key)
            .map_err(|e| blob_failed(&e))?
            .is_some_and(|age| age >= self.stale_millis);
        if !old {
            return Ok(false);
        }
        let Some(held) = held else {
            return Ok(true);
        };
        let group = self.stored(held, marker, doc)?;
        Ok(group.is_none_or(|g| g.get("name").and_then(Value::as_str) != Some(name)))
    }

    /// The id holding the claim on `name`, if one does (1 GET).
    pub(crate) fn holder(&self, name: &str) -> Result<Option<String>, RestError> {
        let mut bytes: Vec<u8> = Vec::new();
        let mut buf = [0u8; CLAIM_BUFFER];
        let mut sink = |chunk: &[u8]| {
            if bytes.len().saturating_add(chunk.len()) <= CLAIM_BUFFER {
                bytes.extend_from_slice(chunk);
            }
            Ok(())
        };
        match self
            .store
            .get(&self.claim_key(name), ByteRange::Full, &mut buf, &mut sink)
        {
            Ok(_) => Ok(String::from_utf8(bytes).ok().filter(|b| !b.is_empty())),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(blob_failed(&e)),
        }
    }

    /// Deletes the claim on `name` if it still holds `id` (`None` for a claim
    /// that holds nothing readable), so it never removes another group's claim.
    pub(super) fn release(&self, name: &str, id: Option<&str>) {
        if self.holder(name).ok().is_some_and(|h| h.as_deref() == id) {
            let _ = self.store.delete(&self.claim_key(name));
        }
    }
}
