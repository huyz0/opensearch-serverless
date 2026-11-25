//! The proactive takeover's reaper-selection switch.

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
