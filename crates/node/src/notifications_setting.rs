//! Whether this node serves the notifications plugin.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.notifications.enabled`: serve `/_plugins/_notifications`, the
/// stock plugin's channel config routes, natively. Default `true`, as for every
/// plugin; with it off those paths answer as an unknown route.
pub const NOTIFICATIONS_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.notifications.enabled",
    true,
    Scope::Node,
    "Serve the notifications plugin's channel config routes over document sets in the object \
     store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[NOTIFICATIONS_ENABLED.setting()];
