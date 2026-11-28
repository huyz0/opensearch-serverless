//! The slow start of a joining reader (docs/specs/reader-slow-start).

use oss_types::settings::{IntSetting, Scope, Setting};

/// `serverless.search.reader_ramp.max_duration` (`OSS_READER_RAMP_MAX_SECS`):
/// seconds by which a joining reader is fully ramped, whatever it says.
/// Default 300; 0 turns the slow start off and every reader takes its full
/// share at once.
pub const RAMP_MAX_DURATION: IntSetting = IntSetting::new(
    "serverless.search.reader_ramp.max_duration",
    300,
    0,
    86_400,
    Scope::Node,
    "Seconds by which a joining reader takes its full share of shards; 0 turns the slow start off.",
);

/// `serverless.search.reader_ramp.min_duration` (`OSS_READER_RAMP_MIN_SECS`):
/// the soonest a joining reader may be fully ramped, however warm it says it is.
pub const RAMP_MIN_DURATION: IntSetting = IntSetting::new(
    "serverless.search.reader_ramp.min_duration",
    30,
    0,
    86_400,
    Scope::Node,
    "Seconds before which a joining reader is not fully ramped, however warm it says it is.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[RAMP_MAX_DURATION.setting(), RAMP_MIN_DURATION.setting()];
