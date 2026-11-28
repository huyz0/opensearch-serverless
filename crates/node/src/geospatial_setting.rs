//! Whether this node's index admin accepts a `geo_point` field in a mapping.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.geospatial.index_fields`: whether a create-index request may
/// map a `geo_point` field, which is then indexed as `LatLonPoint`s and doc
/// values and whose malformed values are refused (unless `ignore_malformed`).
/// Default `true`: a ported plugin's feature needs no flag, or one that is on.
/// Indexing adds index size and index-time work, so the figure on a geo
/// corpus is to be recorded and a result over 5% reopens the default. Off, an
/// index with one is refused as unsupported and stays on a Java writer.
pub const GEOSPATIAL_INDEX_FIELDS: BoolSetting = BoolSetting::new(
    "serverless.geospatial.index_fields",
    true,
    Scope::Node,
    "Accept the geo_point field type in a created index's mapping and index its values as points and doc values.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[GEOSPATIAL_INDEX_FIELDS.setting()];
