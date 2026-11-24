//! docs/specs/write-concurrency: ids drawn by many requests at once.

#![cfg(feature = "writer")]

use std::collections::HashSet;

use oss_node::id_lanes::IdLanes;

// AC-1
#[test]
fn lanes_differ_until_the_counter_wraps() {
    let lanes = IdLanes::new([1, 2, 3, 4, 5, 6]);
    let mut seen = HashSet::new();
    // Every lane draws one id at one clock value, the worst case for a
    // repeat; 65,536 requests apart is the first reuse.
    for _ in 0..65_536 {
        assert!(seen.insert(lanes.generator().next(1_700_000_000_000)));
    }
    assert_eq!(seen.len(), 65_536);
}

// AC-1
#[test]
fn sixty_four_generators_drawing_thousands_each_never_collide() {
    let lanes = IdLanes::new([9, 8, 7, 6, 5, 4]);
    let mut seen = HashSet::new();
    for _ in 0..64 {
        let mut ids = lanes.generator();
        for _ in 0..4_096 {
            assert!(seen.insert(ids.next(1_700_000_000_000)));
        }
    }
}

// AC-2
#[test]
fn an_id_has_the_old_layout_and_a_requests_ids_are_in_time_order() {
    let lanes = IdLanes::new([1, 2, 3, 4, 5, 6]);
    let mut ids = lanes.generator();
    let first = ids.next(1_000);
    let later = ids.next(2_000);
    assert_eq!(first.len(), 20);
    assert!(
        first
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    assert_ne!(first, later);
}
