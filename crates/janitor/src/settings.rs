//! The janitor's settings.

use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.janitor.dry_run`: count what a sweep would delete and delete
/// nothing. On until the fault suites show no live blob is ever counted.
pub const DRY_RUN: BoolSetting = BoolSetting::new(
    "serverless.janitor.dry_run",
    true,
    Scope::Node,
    "Count the blobs a janitor sweep would delete, and delete none.",
)
.dynamic();

/// Every setting this crate defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[DRY_RUN.setting()];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_janitor_starts_as_a_dry_run() {
        assert!(DRY_RUN.default());
    }
}
