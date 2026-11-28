//! Whether this node serves the security configuration API.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.security.enabled`: serve `/_plugins/_security/api/...`.
/// With it on, every request is authenticated and authorized against the
/// stored configuration. The node turns it on when an administrator is configured (`OSS_SECURITY_ADMIN_USER`), so no flag is needed; with none a node is open.
pub const SECURITY_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.security.enabled",
    false,
    Scope::Node,
    "Serve the security plugin's roles, role mapping, internal user, tenant and action group \
     management API over document sets in the object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[SECURITY_ENABLED.setting()];

/// The static administrator from its two variables: both or neither.
///
/// # Errors
///
/// A message when only one is set.
pub fn admin_pair(
    user: Option<String>,
    hash: Option<String>,
) -> Result<Option<(String, String)>, String> {
    match (user, hash) {
        (Some(user), Some(hash)) => Ok(Some((user, hash))),
        (None, None) => Ok(None),
        _ => Err("OSS_SECURITY_ADMIN_USER and OSS_SECURITY_ADMIN_HASH go together".to_owned()),
    }
}
