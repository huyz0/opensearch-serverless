//! The commit staging variables.
use super::tests::{FS, lookup, with};
use super::*;

#[test]
fn commit_staging_is_off_by_default_and_set_by_its_two_variables() {
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(off.commit_checkpoint_millis, None);
    let on =
        NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_STAGING", "true")]))).expect("valid");
    assert_eq!(on.commit_checkpoint_millis, Some(60_000));
    let aged = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_COMMIT_STAGING", "true"),
        ("OSS_COMMIT_CHECKPOINT_MS", "2000"),
    ])))
    .expect("valid");
    assert_eq!(aged.commit_checkpoint_millis, Some(2_000));
    let age_alone = NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_CHECKPOINT_MS", "5000")])))
        .expect("valid");
    assert_eq!(
        age_alone.commit_checkpoint_millis, None,
        "the switch is off"
    );
}

#[test]
fn a_checkpoint_age_outside_its_bounds_is_refused() {
    for age in ["1999", "150001"] {
        let refused = NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_CHECKPOINT_MS", age)])));
        assert!(refused.is_err(), "{age}");
    }
    for age in ["2000", "150000"] {
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_CHECKPOINT_MS", age)]))).is_ok()
        );
    }
}

// AC-8
#[test]
fn commit_logging_is_off_by_default_and_needs_staging() {
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(!off.commit_logged);
    let on = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_COMMIT_STAGING", "true"),
        ("OSS_COMMIT_LOGGED", "true"),
    ])))
    .expect("valid");
    assert!(on.commit_logged);
    let alone = NodeConfig::from_lookup(lookup(&with(&[("OSS_COMMIT_LOGGED", "true")])));
    assert!(
        alone
            .expect_err("refused")
            .contains("needs OSS_COMMIT_STAGING")
    );
}
