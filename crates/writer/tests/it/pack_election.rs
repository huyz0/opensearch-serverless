//! Which node runs a collection window's pass: one member, chosen once for a
//! term of windows by rendezvous hashing over the live membership, with no
//! lease.

use oss_writer::pack::{TERM_WINDOWS, WINDOW_MILLIS, collector_for, term_of, window_of};

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
    for term in 0..200 {
        let window = term * TERM_WINDOWS;
        let a = collector_for(window, &ids).expect("some collector");
        let b = collector_for(window, &ids).expect("some collector");
        assert_eq!(a, b);
        assert!(ids.iter().any(|id| id == a));
        seen.insert(a.to_owned());
    }
    assert_eq!(seen.len(), 5, "every member takes its term");
}

/// Nobody collects with no members; one member always collects.
#[test]
fn an_empty_membership_has_no_collector_and_one_member_is_always_it() {
    assert_eq!(collector_for(7, &[]), None);
    let one = members(1);
    for window in 0..20 {
        assert_eq!(
            collector_for(window * TERM_WINDOWS, &one),
            Some("node-0000")
        );
    }
}

/// The choice is pinned, so every version of the node agrees: not Rust's
/// hasher, which may change between versions.
#[test]
fn the_choice_is_a_pinned_function_of_the_window() {
    let ids = members(10);
    let picks: Vec<&str> = (0..6)
        .map(|t| collector_for(t * TERM_WINDOWS, &ids).expect("some collector"))
        .collect();
    assert_eq!(
        picks,
        vec![
            "node-0009",
            "node-0008",
            "node-0009",
            "node-0008",
            "node-0003",
            "node-0000"
        ],
        "pinned: a change moves who collects each term"
    );
}

/// A term is a day of windows, and the one member runs every window of it.
#[test]
fn one_member_runs_every_window_of_a_term() {
    assert_eq!(TERM_WINDOWS, 144);
    assert_eq!(term_of(0), 0);
    assert_eq!(term_of(TERM_WINDOWS - 1), 0);
    assert_eq!(term_of(TERM_WINDOWS), 1);
    let ids = members(50);
    for term in 0..20 {
        let first = collector_for(term * TERM_WINDOWS, &ids);
        for w in 0..TERM_WINDOWS {
            assert_eq!(collector_for(term * TERM_WINDOWS + w, &ids), first);
        }
    }
}

/// A member leaving moves the choice only if it was the collector, and a
/// member joining moves it only to that member: churn elsewhere in a big
/// fleet does not hand the pass to a cold node.
#[test]
fn the_choice_moves_only_for_the_member_that_left_or_joined() {
    let ids = members(40);
    for term in 0..100 {
        let window = term * TERM_WINDOWS;
        let chosen = collector_for(window, &ids)
            .expect("some collector")
            .to_owned();
        for out in ids.iter().filter(|id| **id != chosen) {
            let fewer: Vec<String> = ids.iter().filter(|id| *id != out).cloned().collect();
            assert_eq!(collector_for(window, &fewer), Some(chosen.as_str()));
        }
        let mut more = ids.clone();
        more.push("zz-new".to_owned());
        more.sort();
        let now = collector_for(window, &more).expect("some collector");
        assert!(now == chosen || now == "zz-new");
    }
}
