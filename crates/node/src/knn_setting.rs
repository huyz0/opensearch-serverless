//! Whether this node's index admin accepts a `knn_vector` field (the shell's
//! k-NN plugin being installed).

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.knn.enabled`: whether a create-index request may map a
/// `knn_vector` field. Default `false`: the type is indexed but no `knn`
/// query serves it yet, so an index with one stays refused as unsupported
/// (and on a Java writer) until the query exists.
pub const KNN_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.knn.enabled",
    false,
    Scope::Node,
    "Accept the knn_vector field type in a created index's mapping and index its vectors. \
     Stands for index.knn being true until index settings can be given at create.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[KNN_ENABLED.setting()];
