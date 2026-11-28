//! Whether this node serves asynchronous search.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.async_search.enabled`: serve `/_plugins/_asynchronous_search`
/// and `/_opendistro/_asynchronous_search`, the stock plugin's routes,
/// natively, with results held in this node's memory. On by default, as every
/// plugin is; `OSS_ASYNC_SEARCH=false` turns it off, and those paths then
/// answer as an unknown route.
pub const ASYNC_SEARCH_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.async_search.enabled",
    true,
    Scope::Node,
    "Serve the asynchronous search plugin's routes, with results kept in this node's memory.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[ASYNC_SEARCH_ENABLED.setting()];
