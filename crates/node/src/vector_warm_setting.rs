//! Whether the engine reads a segment's vector files whole when it opens.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.reader.vector_warm.at_open` (`OSS_VECTOR_WARM_AT_OPEN`):
/// whether a segment's vector files are read whole when it opens. Default
/// `false`: they are read in one batch by the first `knn` search instead, so
/// a term search of an index with vectors reads none of them, and a `knn`
/// search reads the same bytes in fewer requests
/// (docs/specs/reader-vector-warm). Node scope only: the engine's read
/// advice is process-wide.
pub const VECTOR_WARM_AT_OPEN: BoolSetting = BoolSetting::new(
    "serverless.reader.vector_warm.at_open",
    false,
    Scope::Node,
    "Read a segment's vector files whole when it opens, not at the first knn search.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[VECTOR_WARM_AT_OPEN.setting()];
