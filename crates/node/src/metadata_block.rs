//! The `index.blocks.metadata` block: whether an index's stored settings hold
//! it, and the refusal a metadata change of such an index answers.

use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_meta::{DescriptorStore, MetaError};
use oss_rest::RestError;

use crate::route_util::refuse;
use crate::settings_route::flat_members;
use crate::settings_route::text_of;

/// The key of the block.
pub(crate) const KEY: &str = "index.blocks.metadata";

/// Whether the stored settings set the block `key` to `true`.
pub(crate) fn holds_block(settings: &[u8], key: &str) -> bool {
    let Ok(Json::Obj(top)) = parse_json(settings) else {
        return false;
    };
    // ALLOC: the stored settings, bounded by the descriptor.
    let mut held: Vec<(String, Json)> = Vec::new();
    flat_members(&top, "", &mut held);
    held.iter()
        .any(|(k, v)| k == key && text_of(v).trim() == "true")
}

/// The 403 a metadata change of `index` answers while the block is held.
pub(crate) fn refusal(index: &str) -> RestError {
    refuse(
        403,
        "cluster_block_exception",
        // ALLOC: error path; one message.
        format!("index [{index}] blocked by: [FORBIDDEN/9/index metadata (api)];"),
    )
}

/// Refuses when the stored settings of `index` hold the metadata block.
pub(crate) fn check_settings(index: &str, settings: &[u8]) -> Result<(), RestError> {
    if holds_block(settings, KEY) {
        Err(refusal(index))
    } else {
        Ok(())
    }
}

/// Reads `index`'s descriptor (one GET) and refuses when it holds the block;
/// an index with no descriptor is left to the caller's own not-found answer.
pub(crate) fn check(
    descriptors: &DescriptorStore,
    index: &str,
    buf: &mut [u8],
) -> Result<(), RestError> {
    let read = descriptors
        .get(index, buf)
        .map_err(|e: MetaError| refuse(500, "metadata_block_failure", e.to_string()))?;
    match read {
        Some(record) => check_settings(index, record.settings),
        None => Ok(()),
    }
}
