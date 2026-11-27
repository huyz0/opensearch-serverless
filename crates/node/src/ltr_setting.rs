//! Whether this node serves the learning to rank store.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.ltr.enabled`: serve `/_ltr` and `/_plugins/_ltr`, the stock
/// plugin's feature store, natively. Default `false` until the measurements
/// the spec names are recorded; with it off those paths go to a plugin
/// upstream when one is configured.
pub const LTR_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.ltr.enabled",
    false,
    Scope::Node,
    "Serve the learning to rank plugin's feature store and models over document sets in the \
     object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[LTR_ENABLED.setting()];
