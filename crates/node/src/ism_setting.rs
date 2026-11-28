//! Whether this node serves index state management.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.ism.enabled`: serve `/_plugins/_ism` and `/_opendistro/_ism`,
/// the stock plugin's policy routes, natively. Default `false` until the
/// measurements the spec names are recorded; with it off those paths go to a
/// plugin upstream when one is configured, else answer as an unknown route.
pub const ISM_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.ism.enabled",
    false,
    Scope::Node,
    "Serve the index state management plugin's policy routes over document sets in the object \
     store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[ISM_ENABLED.setting()];
