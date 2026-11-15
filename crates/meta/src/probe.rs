//! Whether a store honours a conditional delete, learned by asking it.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ConditionalDeleteProbe.java`.
//! An S3-compatible store may ignore `If-Match` on DELETE and remove the
//! object regardless; relying on it there would let a late delete remove a
//! name created again. So before a store's deletes are trusted to be
//! conditional, a probe register is moved behind this store's back and a
//! delete at its old generation must be refused.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use oss_blob::{BlobStore, SliceSource, SwapOutcome};
use oss_format::GENERATION_BYTES;

use crate::descriptors::DeleteMode;

/// Where under a deployment's base the probe writes (`RegisterMap.probe`).
pub const PROBE_PREFIX: &str = "probe/";

impl DeleteMode {
    /// [`DeleteMode::Conditional`] if `store` refused a delete at a
    /// generation its register no longer held and then applied one at the
    /// current generation; [`DeleteMode::Tombstone`] otherwise, including
    /// when any request fails, as the Java's probe answers `false`.
    ///
    /// The probe's register is under `<base>probe/` and is deleted at the
    /// end whatever the answer.
    pub fn probe(store: &dyn BlobStore, base: &str) -> Self {
        // ALLOC: one key a probe, run once a store.
        let key = format!(
            "{base}{PROBE_PREFIX}conditional-delete-{:016x}",
            RandomState::new().hash_one(0u8)
        );
        let honoured = honoured(store, &key).unwrap_or(false);
        // The probe's own register; left behind it harms nothing.
        let _ = store.delete(&key);
        if honoured {
            Self::Conditional
        } else {
            Self::Tombstone
        }
    }
}

fn honoured(store: &dyn BlobStore, key: &str) -> Result<bool, oss_blob::BlobError> {
    let SwapOutcome::Applied(created) = store.create_register(key, b"created")? else {
        return Ok(false);
    };
    // Read back, so a store that remembers versions knows this one's tag
    // even when its create's response carried none.
    let mut buf = [0u8; 64];
    if store.read_register(key, &mut buf)?.map(|r| r.generation) != Some(created) {
        return Ok(false);
    }
    // Another writer moves the register on, with a plain PUT this store does
    // not take for its own swap: it still remembers the version it created.
    let moved = created.saturating_add(1);
    let mut body = [0u8; GENERATION_BYTES + 5];
    let (generation, value) = body.split_at_mut(GENERATION_BYTES);
    generation.copy_from_slice(&moved.to_be_bytes());
    value.copy_from_slice(b"moved");
    store.put(key, &mut SliceSource::new(&body), &mut [0u8; 64])?;
    if store.delete_register(key, created)? {
        // The store deleted a register at a generation it no longer held.
        return Ok(false);
    }
    if store.read_register(key, &mut buf)?.map(|r| r.generation) != Some(moved) {
        return Ok(false);
    }
    Ok(store.delete_register(key, moved)? && store.read_register(key, &mut buf)?.is_none())
}
