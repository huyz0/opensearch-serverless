//! Whether this node serves the learning to rank store, and its bounds.

use oss_types::settings::{BoolSetting, IntSetting, LongSetting, Scope, Setting};

/// `serverless.ltr.enabled`: serve `/_ltr` and `/_plugins/_ltr`, the stock
/// plugin's feature store, natively, and `sltr` rescores. Default `true`
/// since the measurements the spec names were recorded; with it off those
/// paths answer as an unknown route.
pub const LTR_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.ltr.enabled",
    true,
    Scope::Node,
    "Serve the learning to rank plugin's feature store, models and sltr rescore over document \
     sets in the object store.",
);

/// `serverless.ltr.max_window`: the most hits an `sltr` rescore scores.
pub const LTR_MAX_WINDOW: IntSetting = IntSetting::new(
    "serverless.ltr.max_window",
    1_000,
    1,
    10_000,
    Scope::Node,
    "The most hits one sltr rescore scores; a larger window is refused.",
);

/// `serverless.ltr.cache_ttl`: seconds a cached model is used before one
/// read of its set's marker says whether it moved.
pub const LTR_CACHE_TTL: IntSetting = IntSetting::new(
    "serverless.ltr.cache_ttl",
    30,
    1,
    3_600,
    Scope::Node,
    "Seconds a cached model is used before its set's marker is read again.",
);

/// `serverless.ltr.cache_bytes`: what the model cache may hold.
pub const LTR_CACHE_BYTES: LongSetting = LongSetting::new(
    "serverless.ltr.cache_bytes",
    64 << 20,
    1 << 20,
    4 << 30,
    Scope::Node,
    "The most bytes the learning to rank model cache holds, reserved from the open-indexes pool.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    LTR_ENABLED.setting(),
    LTR_MAX_WINDOW.setting(),
    LTR_CACHE_TTL.setting(),
    LTR_CACHE_BYTES.setting(),
];
