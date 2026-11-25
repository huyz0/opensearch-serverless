//! A restore's `index_settings` and `ignore_index_settings` applied to the
//! settings an index was captured with, checked before anything is written.

use oss_coordinator::snapshot_restore_settings::SettingsChange;
use oss_rest::RestError;

use crate::create_settings::{checked, refuse_others};
use crate::route_util::refused;

/// The settings an index of `shards` is restored with: `captured` changed as
/// `change` says. A request that names neither key returns `captured`
/// untouched, so its descriptor bytes are the same as before the keys existed.
///
/// # Errors
///
/// A fixed setting named, a key this node does not read (501), a value that
/// does not check (400), or captured settings that are not flat text.
pub(crate) fn apply(
    change: &SettingsChange,
    captured: Option<Vec<u8>>,
    shards: u32,
) -> Result<Option<Vec<u8>>, RestError> {
    if change.is_empty() {
        return Ok(captured);
    }
    refuse_others(&change.set, |_| false)?;
    checked(&change.set, shards, false, false)?;
    let merged = change.merge(captured.as_deref()).map_err(refused)?;
    Ok(Some(merged))
}
