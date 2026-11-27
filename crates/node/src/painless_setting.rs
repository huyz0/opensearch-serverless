//! Whether the node compiles Painless source itself.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.scripting.painless.native_compile`
/// (`OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE`): whether a Painless script is
/// compiled by this node's Rust front end and run by its engine. Default
/// `false`: a Painless script is refused, as an expression-only node does
/// (docs/adr/0069). Node scope only: the switch is process-wide.
pub const NATIVE_COMPILE: BoolSetting = BoolSetting::new(
    "serverless.scripting.painless.native_compile",
    false,
    Scope::Node,
    "Compile Painless scripts in this node instead of refusing them.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[NATIVE_COMPILE.setting()];
