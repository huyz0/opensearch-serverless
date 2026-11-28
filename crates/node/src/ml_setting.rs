//! Whether this node serves ML Commons.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.ml.enabled`: serve `/_plugins/_ml`, the stock plugin's model
/// registry routes, natively. Default `false` until the measurements the spec
/// names are recorded; with it off those paths go to a plugin upstream when
/// one is configured, else answer as an unknown route.
pub const ML_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.ml.enabled",
    false,
    Scope::Node,
    "Serve the ML Commons plugin's model registry routes over document sets in the object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[ML_ENABLED.setting()];
