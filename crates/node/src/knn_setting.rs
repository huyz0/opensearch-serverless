//! Whether this node's index admin accepts a `knn_vector` field (the shell's
//! k-NN plugin being installed).

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.knn.enabled`: whether a create-index request may map a
/// `knn_vector` field. Default `true`: the `knn` query serves the field, a
/// search adds no request over a non-vector index's (docs/knn-measurements.md),
/// and an index with no vector field is unaffected. Off, an index with one is
/// refused as unsupported and stays on a Java writer.
pub const KNN_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.knn.enabled",
    true,
    Scope::Node,
    "Accept the knn_vector field type in a created index's mapping and index its vectors. \
     Stands for index.knn being true until index settings can be given at create.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[KNN_ENABLED.setting()];
