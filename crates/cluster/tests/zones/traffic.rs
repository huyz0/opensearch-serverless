//! The cross-zone byte meter (AC-3).

use std::sync::Arc;

use oss_cluster::zones::{MAX_ACTIONS, OTHER_ACTION, Path, ZoneTraffic, ZoneTrafficStats};

#[test]
fn an_exchange_is_classified_as_the_javas() {
    assert_eq!(Path::between(Some("a"), Some("a")), Path::SameZone);
    assert_eq!(Path::between(Some("a"), Some("b")), Path::CrossZone);
    assert_eq!(Path::between(None, Some("a")), Path::Unknown);
    assert_eq!(Path::between(Some("a"), None), Path::Unknown);
    assert_eq!(Path::between(None, None), Path::Unknown, "never one zone");
    assert_eq!(Path::ALL, [Path::SameZone, Path::CrossZone, Path::Unknown]);
}

#[test]
fn bytes_are_totalled_and_broken_down_by_action() {
    let meter = ZoneTraffic::new();
    meter.record(Path::CrossZone, "search", 100);
    meter.record(Path::CrossZone, "bulk", 7);
    meter.record(Path::CrossZone, "search", 23);
    meter.record(Path::SameZone, "search", 5);
    meter.record(Path::Unknown, "bulk", 2);
    meter.record(Path::SameZone, "empty", 0);
    assert_eq!(meter.total(Path::CrossZone), 130);
    assert_eq!(meter.total(Path::SameZone), 5);
    assert_eq!(meter.total(Path::Unknown), 2);
    let by: Vec<_> = meter.by_action(Path::CrossZone).into_iter().collect();
    assert_eq!(by, [("bulk".to_owned(), 7), ("search".to_owned(), 123)]);
    assert!(
        !meter.by_action(Path::SameZone).contains_key("empty"),
        "zero bytes count nothing"
    );
    let stats = meter.stats();
    assert_eq!(
        stats,
        ZoneTrafficStats {
            same_zone_bytes: 5,
            cross_zone_bytes: 130,
            unknown_zone_bytes: 2,
            cross_zone_by_action: meter.by_action(Path::CrossZone),
        }
    );
    assert_eq!(
        stats.cross_zone_by_action.values().sum::<u64>(),
        stats.cross_zone_bytes
    );
    meter.reset();
    assert_eq!(meter.stats(), ZoneTrafficStats::default());
    for path in Path::ALL {
        assert_eq!(meter.total(path), 0);
    }
}

#[test]
fn concurrent_counts_are_exact() {
    let meter = Arc::new(ZoneTraffic::new());
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let meter = Arc::clone(&meter);
            std::thread::spawn(move || {
                for i in 0..1_000u64 {
                    let action = if i % 2 == 0 { "search" } else { "bulk" };
                    meter.record(Path::CrossZone, action, t + 1);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    // Each thread t counts 1,000 times t + 1 bytes.
    assert_eq!(meter.total(Path::CrossZone), 1_000 * (1..=8).sum::<u64>());
    assert_eq!(meter.by_action(Path::CrossZone)["search"], 500 * 36);
}

#[test]
fn past_the_action_bound_bytes_go_to_other_and_the_total_stays_exact() {
    let meter = ZoneTraffic::new();
    for i in 0..MAX_ACTIONS + 10 {
        meter.record(Path::CrossZone, &format!("a{i:03}"), 1);
    }
    meter.record(Path::CrossZone, "a000", 4);
    let by = meter.by_action(Path::CrossZone);
    assert_eq!(by.len(), MAX_ACTIONS + 1);
    assert_eq!(by[OTHER_ACTION], 10);
    assert_eq!(
        by["a000"], 5,
        "a known action keeps counting under its name"
    );
    assert!(!by.contains_key(&format!("a{:03}", MAX_ACTIONS)));
    assert_eq!(meter.total(Path::CrossZone), (MAX_ACTIONS + 14) as u64);
    assert_eq!(meter.total(Path::SameZone), 0, "the bound is per path");
    meter.record(Path::SameZone, "fresh", 3);
    assert_eq!(meter.by_action(Path::SameZone)["fresh"], 3);
}
