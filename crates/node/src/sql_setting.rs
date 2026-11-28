//! Whether this node serves SQL, and its bounds.

use oss_types::settings::{BoolSetting, IntSetting, Scope, Setting};

/// `serverless.sql.enabled`: serve `/_plugins/_sql` natively. Off until the
/// spec's evidence is recorded; with it off the routes answer the plugin's
/// disabled error.
pub const SQL_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.sql.enabled",
    false,
    Scope::Node,
    "Serve the SQL endpoint, parsed and planned natively into the search path.",
);

/// `serverless.sql.size_limit`: the most rows a statement returns.
pub const SQL_SIZE_LIMIT: IntSetting = IntSetting::new(
    "serverless.sql.size_limit",
    200,
    1,
    10_000,
    Scope::Node,
    "The most rows one SQL statement returns; a LIMIT past it is cut to it.",
);

/// `serverless.sql.max_request_bytes`: the largest SQL request body.
pub const SQL_MAX_REQUEST_BYTES: IntSetting = IntSetting::new(
    "serverless.sql.max_request_bytes",
    1 << 20,
    1_024,
    16 << 20,
    Scope::Node,
    "The largest SQL request body read; a larger one is refused with 413.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    SQL_ENABLED.setting(),
    SQL_SIZE_LIMIT.setting(),
    SQL_MAX_REQUEST_BYTES.setting(),
];
