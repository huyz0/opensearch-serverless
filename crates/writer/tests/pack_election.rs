//! Which node runs a collection window's pass: one member, chosen by the
//! window number over the sorted live membership, with no lease.

use oss_writer::pack::{WINDOW_MILLIS, collector_for, window_of};

fn members(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("node-{i:04}")).collect()
}

/// A window is ten minutes of the store's clock.
#[test]
fn a_window_is_ten_minutes() {
    assert_eq!(WINDOW_MILLIS, 600_000);
    assert_eq!(window_of(0), 0);
    assert_eq!(window_of(599_999), 0);
    assert_eq!(window_of(600_000), 1);
    assert_eq!(window_of(u64::MAX), u64::MAX / 600_000);
}

/// Nodes that agree on the membership agree on one collector, and every
/// member is chosen for some window.
#[test]
fn members_that_agree_pick_the_same_one_collector() {
    let ids = members(5);
    let mut seen = std::collections::BTreeSet::new();
    for window in 0..200 {
        let a = collector_for(window, &ids).expect("some collector");
        let b = collector_for(window, &ids).expect("some collector");
        assert_eq!(a, b);
        assert!(ids.iter().any(|id| id == a));
        seen.insert(a.to_owned());
    }
    assert_eq!(seen.len(), 5, "every member takes its turn");
}

/// Nobody collects with no members; one member always collects.
#[test]
fn an_empty_membership_has_no_collector_and_one_member_is_always_it() {
    assert_eq!(collector_for(7, &[]), None);
    let one = members(1);
    for window in 0..20 {
        assert_eq!(collector_for(window, &one), Some("node-0000"));
    }
}

/// The choice is pinned, so every version of the node agrees: not Rust's
/// hasher, which may change between versions.
#[test]
fn the_choice_is_a_pinned_function_of_the_window() {
    let ids = members(10);
    let picks: Vec<&str> = (0..6)
        .map(|w| collector_for(w, &ids).expect("some collector"))
        .collect();
    assert_eq!(
        picks,
        vec![
            "node-0005",
            "node-0004",
            "node-0003",
            "node-0002",
            "node-0001",
            "node-0000"
        ],
        "pinned: a change moves who collects each window"
    );
}
