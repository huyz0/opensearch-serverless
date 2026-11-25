//! The cluster setting that lets a create pin an analyzer module
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)).

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.extensions.pins.allowed`: whether a create may carry an
/// analyzer pin. Default `false`: an index that pins a module cannot be
/// served by a Java node, so an operator turns this on only when every node
/// that can hold a shard of the index runs the runtime. Turning it off
/// refuses new pins only; existing pins keep working.
pub const PINS_ALLOWED: BoolSetting = BoolSetting::new(
    "serverless.extensions.pins.allowed",
    false,
    Scope::Node,
    "Let a create-index request pin a WASM module as a named analyzer. Turn on only when \
     every node that can hold a shard of such an index runs the extension runtime.",
)
.dynamic();

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[PINS_ALLOWED.setting()];
