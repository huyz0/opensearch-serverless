//! Whether this node serves the report definitions.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.reports.enabled`: serve `/_plugins/_reports/definition`, the
/// stock reporting plugin's report definitions, natively. Default `true` (a
/// ported plugin needs no flag, or its flag is on by default); with it off
/// those paths answer as an unknown route.
pub const REPORTS_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.reports.enabled",
    true,
    Scope::Node,
    "Serve the reporting plugin's report definition routes over a document set in the object store.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[REPORTS_ENABLED.setting()];
