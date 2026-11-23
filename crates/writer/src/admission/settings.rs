//! Settings of the write limiter (the Java's `WriteSettings` backpressure
//! group), with the Java's defaults.

use oss_types::settings::{IntSetting, LongSetting, Scope, Setting};

/// `serverless.write.backpressure.target_millis`: an append slower than
/// this is a sign to back off. Default 1,000.
pub const TARGET_MILLIS: LongSetting = LongSetting::new(
    "serverless.write.backpressure.target_millis",
    1_000,
    1,
    3_600_000,
    Scope::Node,
    "How long a write-log append may take, in milliseconds, before the node lowers how many \
     writes it lets wait on the object store.",
);

/// `serverless.write.backpressure.budget_millis`: while appends take longer
/// than this, new writes beyond the minimum are refused. Default 5,000.
pub const BUDGET_MILLIS: LongSetting = LongSetting::new(
    "serverless.write.backpressure.budget_millis",
    5_000,
    1,
    3_600_000,
    Scope::Node,
    "While write-log appends take longer than this, in milliseconds, the node refuses new \
     writes beyond the minimum in flight with a 429 and a retry hint.",
);

/// `serverless.write.backpressure.min_in_flight`: the fewest writes ever let
/// through at once. Default 4.
pub const MIN_IN_FLIGHT: IntSetting = IntSetting::new(
    "serverless.write.backpressure.min_in_flight",
    4,
    1,
    i32::MAX,
    Scope::Node,
    "The fewest writes the node lets wait on the object store at once, however slow it is.",
);

/// `serverless.write.backpressure.max_in_flight`: the most. Default 1,024.
pub const MAX_IN_FLIGHT: IntSetting = IntSetting::new(
    "serverless.write.backpressure.max_in_flight",
    1_024,
    1,
    i32::MAX,
    Scope::Node,
    "The most writes the node lets wait on the object store at once; the limit starts here and \
     falls while the store is slow. Set very high to never limit.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    TARGET_MILLIS.setting(),
    BUDGET_MILLIS.setting(),
    MIN_IN_FLIGHT.setting(),
    MAX_IN_FLIGHT.setting(),
];
