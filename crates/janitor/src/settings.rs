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

/// Every setting this crate defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[DRY_RUN.setting(), MINIMUM_UNREFERENCED_MILLIS.setting()];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_janitor_starts_as_a_dry_run() {
        assert!(DRY_RUN.default());
    }

    #[test]
    fn a_blob_must_be_unreferenced_for_a_minute_by_default() {
        assert_eq!(MINIMUM_UNREFERENCED_MILLIS.default(), 60_000);
    }
}
