//! The alerts of one monitor: one register holding every alert the monitor
//! has open, as the JSON the plugin's alerts API answers. The register is
//! the node's to read and write as text; this module owns its key and bound.

use crate::error::{FormatError, corrupt};
use crate::monitor_index::{check_base, check_id};
use crate::schedule_registry::check_tenant_id;

const WHAT: &str = "alerting alert set";

/// The longest alert set: a monitor's 100 triggers, each with its alert, its
/// action results and its error text.
pub const MAX_ALERT_SET_BYTES: usize = 1 << 20;

/// The key of a monitor's alert register.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, an invalid tenant id or an invalid monitor id.
pub fn alert_set_key(base: &str, tenant: &str, monitor: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_tenant_id(tenant)?;
    check_id(monitor)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, "alerting/", tenant, "/alerts/", monitor].concat())
}

/// Whether `value` may be stored as an alert set: UTF-8 within the bound.
///
/// # Errors
///
/// [`FormatError::TooLarge`] past [`MAX_ALERT_SET_BYTES`];
/// [`FormatError::Corrupt`] for bytes that are not UTF-8.
pub fn check_alert_set(value: &[u8]) -> Result<(), FormatError> {
    if value.len() > MAX_ALERT_SET_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_ALERT_SET_BYTES as u64,
            actual: value.len() as u64,
        });
    }
    std::str::from_utf8(value)
        .map(|_| ())
        .map_err(|_| corrupt(WHAT, "an alert set is UTF-8"))
}
