//! Settings of the node write log and its group commit (the Java's
//! `NodeLogSettings`), with the Java's defaults.

use oss_types::settings::{BoolSetting, ChoiceSetting, IntSetting, LongSetting, Scope, Setting};

/// `serverless.wal.node_log.window_millis`: how long a window stays open,
/// one of 100, 200 or 250 ms (the Java's `ALLOWED_WINDOW_MILLIS`, checked
/// at start). Default 250.
pub const WINDOW_MILLIS: ChoiceSetting = ChoiceSetting::new(
    "serverless.wal.node_log.window_millis",
    "250",
    &["100", "200", "250"],
    Scope::Node,
    "How long a node's write-log window stays open after its first record, in milliseconds: \
     every record of any shard arriving within it rides one PUT.",
);

/// `serverless.wal.node_log.max_bytes`: roughly the most bytes of id and
/// source one window takes. Default 8 MiB.
pub const MAX_BYTES: LongSetting = LongSetting::new(
    "serverless.wal.node_log.max_bytes",
    8 * 1024 * 1024,
    1,
    i32::MAX as i64,
    Scope::Node,
    "Roughly the most bytes of document id and source one write-log window takes before \
     the next record opens another.",
);

/// `serverless.wal.node_log.max_records`: the most records one window
/// takes. Default 50,000.
pub const MAX_RECORDS: IntSetting = IntSetting::new(
    "serverless.wal.node_log.max_records",
    50_000,
    1,
    i32::MAX,
    Scope::Node,
    "The most records one write-log window takes before the next record opens another.",
);

/// `serverless.wal.node_log.hedge_after_millis`: how long a window's PUT
/// may go unanswered before it is sent again; zero never sends a second.
/// Default 60.
pub const HEDGE_AFTER_MILLIS: LongSetting = LongSetting::new(
    "serverless.wal.node_log.hedge_after_millis",
    60,
    0,
    60_000,
    Scope::Node,
    "How long a write-log PUT may go unanswered before the same PUT is sent again, in \
     milliseconds, raised to the 95th percentile of recent PUTs; 0 never sends a second.",
);

/// `serverless.wal.node_log.idle_flush`: a record finding the log quiet
/// for a window is written at once. Default `false`.
pub const IDLE_FLUSH: BoolSetting = BoolSetting::new(
    "serverless.wal.node_log.idle_flush",
    false,
    Scope::Node,
    "Pace write-log windows by the last blob rather than by their first record, so a record \
     arriving after a quiet window is written at once; costs up to half as many blobs again.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    WINDOW_MILLIS.setting(),
    MAX_BYTES.setting(),
    MAX_RECORDS.setting(),
    HEDGE_AFTER_MILLIS.setting(),
    IDLE_FLUSH.setting(),
];

#[cfg(test)]
mod tests {
    use oss_types::settings::Registry;

    use super::*;

    #[test]
    fn the_node_log_settings_form_a_valid_registry_with_the_javas_defaults() {
        let registry = Registry::new(&[SETTINGS]).expect("valid");
        assert_eq!(registry.all().count(), 5);
        assert_eq!(WINDOW_MILLIS.default(), "250");
        assert_eq!(MAX_BYTES.default(), 8 * 1024 * 1024);
        assert_eq!(MAX_RECORDS.default(), 50_000);
        assert_eq!(HEDGE_AFTER_MILLIS.default(), 60);
        assert!(!IDLE_FLUSH.default());
        assert!(
            WINDOW_MILLIS.parse("300").is_err(),
            "only the Java's windows"
        );
    }
}
