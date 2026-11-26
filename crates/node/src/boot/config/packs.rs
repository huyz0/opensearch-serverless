//! The node pack variables.

use std::time::Duration;

use crate::writer::PackConfig;

/// `OSS_NODE_PACKS` (`serverless.segments.node_pack.enabled`) with
/// `OSS_NODE_PACK_MAX_BLOB_BYTES` and `OSS_NODE_PACK_LINGER_MS`: how a node
/// packs when the switch is on, else none. The sizes are checked either way.
///
/// # Errors
///
/// A message naming a variable out of bounds.
pub(super) fn read(
    flag: &impl Fn(&str, &str) -> Result<bool, String>,
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<Option<PackConfig>, String> {
    let max_blob_bytes = bounded("OSS_NODE_PACK_MAX_BLOB_BYTES", "8388608", 1, 64 << 20)?;
    let linger = bounded("OSS_NODE_PACK_LINGER_MS", "50", 1, 5_000)?;
    Ok(flag("OSS_NODE_PACKS", "false")?.then_some(PackConfig {
        max_blob_bytes,
        linger: Duration::from_millis(linger),
    }))
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
