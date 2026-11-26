//! Whether this node's index admin accepts a `nested` object in a mapping.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.nested.enabled`: whether a create-index request may map a
/// `nested` object. Default `false`: a nested object writes each of its
/// objects as a child document in the document's block, which changes the
/// index's document count and layout, and a search of such an index skips
/// the children with one more filter. Off, an index with one is refused as
/// unsupported and stays on a Java writer.
pub const NESTED_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.nested.enabled",
    false,
    Scope::Node,
    "Accept the nested object type in a created index's mapping and write its objects as child documents.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[NESTED_ENABLED.setting()];
