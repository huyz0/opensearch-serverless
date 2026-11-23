//! The janitor's settings.

use oss_types::settings::{BoolSetting, LongSetting, Scope, Setting};

/// `serverless.janitor.dry_run`: count what a sweep would delete and delete
/// nothing. On until the fault suites show no live blob is ever counted.
pub const DRY_RUN: BoolSetting = BoolSetting::new(
    "serverless.janitor.dry_run",
    true,
    Scope::Node,
    "Count the blobs a janitor sweep would delete, and delete none.",
)
.dynamic();

/// `serverless.janitor.minimum_unreferenced_millis`: how long a blob must
/// have been seen unreferenced before a sweep may delete it, on top of two
/// sightings. Two back-to-back sweeps are no grace for a reader still
/// serving the previous commit, so the default is the Java's one minute
/// (`NodePackCollector.DEFAULT_MINIMUM_UNREFERENCED_MILLIS`) and it should
/// stay above the readers' refresh backstop.
pub const MINIMUM_UNREFERENCED_MILLIS: LongSetting = LongSetting::new(
    "serverless.janitor.minimum_unreferenced_millis",
    60_000,
    0,
    86_400_000,
    Scope::Node,
    "How long a blob must have been unreferenced before the janitor may delete it.",
)
.dynamic();

/// `serverless.janitor.orphan_slice_budget`: how many shard containers one
/// orphan-sweep slice judges. 0, the default, is off: the sweep makes no
/// request at all ([ADR 0028](../../docs/adr/0028-orphan-shard-sweep.md)).
pub const ORPHAN_SLICE_BUDGET: LongSetting = LongSetting::new(
    "serverless.janitor.orphan_slice_budget",
    0,
    0,
    1_000,
    Scope::Node,
    "Shard containers one orphan-sweep slice judges; 0 turns the sweep off.",
)
.dynamic();

/// `serverless.janitor.orphan_slice_interval_millis`: how often the one node
/// the rendezvous names runs a slice.
pub const ORPHAN_SLICE_INTERVAL_MILLIS: LongSetting = LongSetting::new(
    "serverless.janitor.orphan_slice_interval_millis",
    3_600_000,
    1_000,
    86_400_000,
    Scope::Node,
    "How often a node runs one slice of the orphan-shard sweep.",
)
.dynamic();

/// `serverless.janitor.orphan_max_pins`: the most index incarnations the
/// pins of a slice may name before it stops with nothing deleted.
pub const ORPHAN_MAX_PINS: LongSetting = LongSetting::new(
    "serverless.janitor.orphan_max_pins",
    100_000,
    1,
    10_000_000,
    Scope::Node,
    "The most pinned index incarnations a slice of the orphan sweep holds.",
)
.dynamic();

/// `serverless.janitor.orphan_max_pin_records`: the most live points in time
/// and snapshots a slice reads before it stops with nothing deleted.
pub const ORPHAN_MAX_PIN_RECORDS: LongSetting = LongSetting::new(
    "serverless.janitor.orphan_max_pin_records",
    10_000,
    1,
    1_000_000,
    Scope::Node,
    "The most point-in-time and snapshot records a slice of the orphan sweep reads.",
)
.dynamic();

/// Every setting this crate defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    DRY_RUN.setting(),
    MINIMUM_UNREFERENCED_MILLIS.setting(),
    ORPHAN_SLICE_BUDGET.setting(),
    ORPHAN_SLICE_INTERVAL_MILLIS.setting(),
    ORPHAN_MAX_PINS.setting(),
    ORPHAN_MAX_PIN_RECORDS.setting(),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_janitor_starts_as_a_dry_run() {
        assert!(DRY_RUN.default());
    }

    #[test]
    fn the_orphan_sweep_is_off_by_default() {
        assert_eq!(ORPHAN_SLICE_BUDGET.default(), 0);
    }

    #[test]
    fn a_blob_must_be_unreferenced_for_a_minute_by_default() {
        assert_eq!(MINIMUM_UNREFERENCED_MILLIS.default(), 60_000);
    }
}
