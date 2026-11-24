//! Whether this node serves the security configuration API.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.security.enabled`: serve `/_plugins/_security/api/...`.
/// With it on, every request is authenticated and authorized against the
/// stored configuration. Default `false`: a node with it off is open.
pub const SECURITY_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.security.enabled",
    false,
    Scope::Node,
    "Serve the security plugin's roles, role mapping, internal user, tenant and action group \
     management API over document sets in the object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[SECURITY_ENABLED.setting()];
