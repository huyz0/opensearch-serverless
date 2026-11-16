//! The hot-shard spread's properties (AC-6): random, so checked by what
//! must hold of every answer rather than against a recording.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use oss_cluster::placement::{HotShardSpread, ShardKey, SpreadConfig};

fn spread(target: f64, max: usize) -> (HotShardSpread, Arc<AtomicI64>) {
    let clock = Arc::new(AtomicI64::new(5_000_000_000));
    let nanos = Arc::clone(&clock);
    let spread = HotShardSpread::new(
        SpreadConfig {
            target_per_second: target,
            max_readers: max,
        },
        Arc::new(move || nanos.load(Ordering::Relaxed)),
    )
    .unwrap();
    (spread, clock)
}

#[test]
fn a_bad_config_is_refused_and_the_default_is_the_javas() {
    let config = SpreadConfig::default();
    assert_eq!((config.target_per_second, config.max_readers), (50.0, 4));
    let nanos = || Arc::new(|| 0i64);
    for (target, max) in [(0.0, 4), (-1.0, 4), (f64::NAN, 4), (50.0, 0)] {
        let bad = SpreadConfig {
            target_per_second: target,
            max_readers: max,
        };
        assert!(
            HotShardSpread::new(bad, nanos()).is_none(),
            "{target} {max}"
        );
    }
}

#[test]
fn a_cold_shard_keeps_placements_order() {
    let (spread, _) = spread(50.0, 4);
    let key = ShardKey::new("logs", 0);
    assert_eq!(spread.per_second(key), 0.0);
    assert_eq!(spread.width_of(key), 1);
    let ranked = ["a", "b", "c"];
    for _ in 0..40 {
        assert_eq!(spread.order(key, &ranked), ranked);
    }
}

#[test]
fn the_rate_sets_the_width_and_decays() {
    let (spread, clock) = spread(5.0, 3);
    let key = ShardKey::new("logs", 3);
    for _ in 0..25 {
        spread.order(key, &[]);
    }
    // 25 at one instant: the fast average reads 25 a second.
    assert!((spread.per_second(key) - 25.0).abs() < 1e-9);
    assert_eq!(spread.width_of(key), 3);
    clock.fetch_add(1_000_000_000, Ordering::Relaxed);
    let after = spread.per_second(key);
    assert!(after < 25.0 / 2.0 && after > 2.0, "{after}");
    assert_eq!(spread.width_of(key), 2, "{after}");
    clock.fetch_add(60_000_000_000, Ordering::Relaxed);
    assert_eq!(spread.width_of(key), 1);
    // A clock that steps back neither grows nor resets the rate.
    let before = spread.per_second(key);
    clock.fetch_sub(30_000_000_000, Ordering::Relaxed);
    assert_eq!(spread.per_second(key), before);
}

#[test]
fn a_hot_shard_spreads_over_its_width_and_avoids_the_busier_pick() {
    let (spread, _) = spread(1.0, 2);
    let key = ShardKey::new("logs", 1);
    for _ in 0..10 {
        spread.order(key, &[]);
    }
    let ranked = ["a", "b", "c"];
    let mut firsts = std::collections::HashSet::new();
    for _ in 0..200 {
        let ordered = spread.order_counted(key, &ranked);
        assert_eq!(ordered.len(), 3);
        assert!(ordered[0] == "a" || ordered[0] == "b", "{ordered:?}");
        let mut sorted = ordered.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, ranked, "every reader once");
        firsts.insert(ordered[0]);
    }
    assert_eq!(firsts.len(), 2, "both of the width are chosen");
    // With "a" busy, two picks always see both and prefer "b".
    let running: Vec<_> = (0..3).map(|_| spread.begin("a")).collect();
    assert_eq!(spread.in_flight("a"), 3);
    for _ in 0..50 {
        assert_eq!(spread.order_counted(key, &ranked)[0], "b");
    }
    drop(running);
    assert_eq!(spread.in_flight("a"), 0);
    assert_eq!(spread.in_flight("never"), 0);
    assert_eq!(spread.max_readers(), 2);
}

#[test]
fn past_the_tracking_bound_cold_shards_are_dropped() {
    let (spread, clock) = spread(50.0, 4);
    let total = oss_cluster::placement::MAX_TRACKED;
    for shard in 0..total {
        spread.order(ShardKey::new("cold", i32::try_from(shard).unwrap()), &[]);
    }
    // A minute on, each is far below a tenth of the target.
    clock.fetch_add(60_000_000_000, Ordering::Relaxed);
    let hot = ShardKey::new("hot", 0);
    spread.order(hot, &[]);
    assert!(spread.per_second(hot) > 0.0);
    assert_eq!(spread.per_second(ShardKey::new("cold", 0)), 0.0, "dropped");
}

#[test]
fn a_steady_rate_reads_the_slow_average_once_a_burst_fades() {
    let (spread, clock) = spread(50.0, 4);
    let key = ShardKey::new("logs", 9);
    for _ in 0..25 {
        spread.order(key, &[]);
    }
    // Five seconds on the burst average has fallen by e^5, the slow one by
    // e^0.5: 25 searches over a 10 s decay read 2.5 a second, then 1.52.
    clock.fetch_add(5_000_000_000, Ordering::Relaxed);
    let expected = 2.5 * (-0.5f64).exp();
    assert!(
        (spread.per_second(key) - expected).abs() < 1e-9,
        "{}",
        spread.per_second(key)
    );
}

#[test]
fn below_the_tracking_bound_nothing_is_dropped_and_a_warm_shard_never_is() {
    let (spread, clock) = spread(50.0, 4);
    let cold = ShardKey::new("cold", 0);
    spread.order(cold, &[]);
    clock.fetch_add(60_000_000_000, Ordering::Relaxed);
    spread.order(ShardKey::new("other", 0), &[]);
    assert!(spread.per_second(cold) > 0.0, "kept below the bound");
    // At the bound a warm shard, above a tenth of the target, stays.
    let (spread, clock) = spread_at_bound();
    let warm = ShardKey::new("warm", 0);
    for _ in 0..200 {
        spread.order(warm, &[]);
    }
    clock.fetch_add(10_000_000_000, Ordering::Relaxed);
    let before = spread.per_second(warm);
    assert!(before > 5.0 && before < 500.0, "{before}");
    spread.order(ShardKey::new("new", 0), &[]);
    assert_eq!(spread.per_second(warm), before);
}

/// A spread holding exactly `MAX_TRACKED` shards, all recorded now.
fn spread_at_bound() -> (HotShardSpread, Arc<AtomicI64>) {
    let (spread, clock) = spread(50.0, 4);
    for shard in 0..oss_cluster::placement::MAX_TRACKED - 1 {
        spread.order(ShardKey::new("fill", i32::try_from(shard).unwrap()), &[]);
    }
    (spread, clock)
}
