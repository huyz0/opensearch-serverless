//! The proactive takeover's reaper-selection switch, and the forwarded shard search's settings.

use super::*;

#[test]
fn proactive_reapers_are_of_this_build_by_default_and_the_switch_is_read() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.proactive_rust_reapers_only);
    let off = NodeConfig::from_lookup(lookup(&with(&[(
        "OSS_PROACTIVE_RUST_REAPERS_ONLY",
        "false",
    )])))
    .expect("valid");
    assert!(!off.proactive_rust_reapers_only);
}

#[test]
fn the_forwarded_search_action_is_on_and_each_setting_is_read_and_bounded() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.search_shard_action);
    assert_eq!(
        (
            c.search_shard_max_lease_age_millis,
            c.search_shard_max_concurrent
        ),
        (10_000, 2)
    );
    let set = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_SEARCH_SHARD_ACTION", "false"),
        ("OSS_SEARCH_SHARD_ACTION_MAX_LEASE_AGE_MILLIS", "2500"),
        ("OSS_SEARCH_SHARD_ACTION_MAX_CONCURRENT", "3"),
    ])))
    .expect("valid");
    assert!(!set.search_shard_action);
    assert_eq!(
        (
            set.search_shard_max_lease_age_millis,
            set.search_shard_max_concurrent
        ),
        (2_500, 3)
    );
    for (key, bad) in [
        ("OSS_SEARCH_SHARD_ACTION", "yes"),
        ("OSS_SEARCH_SHARD_ACTION_MAX_LEASE_AGE_MILLIS", "0"),
        ("OSS_SEARCH_SHARD_ACTION_MAX_CONCURRENT", "257"),
    ] {
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[(key, bad)]))).is_err(),
            "{key}"
        );
    }
}
