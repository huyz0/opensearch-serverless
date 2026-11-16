//! docs/specs/settings-registry/SPEC.md: settings parsed with the Java's
//! messages, resolved through layers, collected into a registry.

mod kinds;
mod layers;
mod registry;

use oss_types::settings::{BoolSetting, ChoiceSetting, IntSetting, LongSetting, Scope, StrSetting};

pub(crate) const FLAG: BoolSetting =
    BoolSetting::new("serverless.t.flag", true, Scope::Node, "A flag.");
pub(crate) const COUNT: IntSetting =
    IntSetting::new("serverless.t.count", 8, 1, 64, Scope::Node, "A count.");
pub(crate) const BIG: LongSetting = LongSetting::new(
    "serverless.t.big",
    -5,
    -10,
    i64::MAX,
    Scope::Node,
    "A long.",
);
pub(crate) const NAME: StrSetting =
    StrSetting::new("serverless.t.name", "dflt", Scope::Node, "A name.");
pub(crate) const MODE: ChoiceSetting = ChoiceSetting::new(
    "serverless.t.mode",
    "a",
    &["a", "b"],
    Scope::Node,
    "A mode.",
)
.dynamic();
pub(crate) const INDEX_MODE: ChoiceSetting = ChoiceSetting::new(
    "index.serverless.t.mode",
    "a",
    &["a", "b"],
    Scope::Index,
    "A mode for one index.",
)
.dynamic();
