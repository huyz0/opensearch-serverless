//! Whether this node serves the observability object store.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.observability.enabled`: serve `/_plugins/_observability/object`,
/// the stock plugin's store of saved queries, visualisations, panels and
/// notebooks, natively. Default `false` until the spec's criteria pass; with
/// it off those paths answer as an unknown route.
pub const OBSERVABILITY_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.observability.enabled",
    false,
    Scope::Node,
    "Serve the observability plugin's object routes over a document set in the object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[OBSERVABILITY_ENABLED.setting()];
