//! Whether this node serves anomaly detection.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.ad.enabled`: serve `/_plugins/_anomaly_detection` and
/// `/_opendistro/_anomaly_detection`, the stock plugin's detector routes,
/// natively. Default `true`, as for every plugin; with it off those paths go
/// to a plugin upstream when one is configured, else answer as an unknown
/// route.
pub const AD_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.ad.enabled",
    true,
    Scope::Node,
    "Serve the anomaly detection plugin's detector routes over document sets in the object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[AD_ENABLED.setting()];
