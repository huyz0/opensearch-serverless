use super::*;

const MIB: u64 = 1 << 20;

// docs/specs/reader-warm-copy AC-10
#[test]
fn an_unlimited_rate_never_waits() {
    let rate = ServeRate::unlimited();
    let now = Instant::now();
    for _ in 0..5 {
        assert_eq!(rate.reserve(now, 1 << 40), Duration::ZERO);
    }
}

// docs/specs/reader-warm-copy AC-10
#[test]
fn batches_at_one_instant_wait_for_the_bytes_ahead_of_them() {
    let rate = ServeRate::new(10 * MIB);
    let now = Instant::now();
    assert_eq!(
        rate.reserve(now, 10 * MIB),
        Duration::ZERO,
        "the first is free"
    );
    assert_eq!(rate.reserve(now, 5 * MIB), Duration::from_secs(1));
    assert_eq!(rate.reserve(now, MIB), Duration::from_millis(1500));
    assert_eq!(rate.reserve(now, 0), Duration::from_millis(1600));
}

// docs/specs/reader-warm-copy AC-10
#[test]
fn the_bytes_served_over_a_window_never_exceed_the_rate_by_more_than_a_batch() {
    let rate = ServeRate::new(8 * MIB);
    let start = Instant::now();
    let mut served = 0_u64;
    let mut last_end = Duration::ZERO;
    for i in 0..50_u64 {
        // A burst of joiners asks every 50 ms, 1 MiB each.
        let at = Duration::from_millis(i * 50);
        let wait = rate.reserve(start + at, MIB);
        served += MIB;
        last_end = at + wait;
        // Everything but the last batch was sent by `last_end`.
        let allowed = (8.0 * MIB as f64 * last_end.as_secs_f64()) as u64 + MIB;
        assert!(served <= allowed, "{served} bytes by {last_end:?}");
    }
    assert!(
        last_end >= Duration::from_secs(5),
        "50 MiB at 8 MiB/s takes 6 s"
    );
}

// docs/specs/reader-warm-copy AC-10
#[test]
fn one_joiner_at_the_rate_is_never_made_to_wait() {
    let rate = ServeRate::new(64 * MIB);
    let start = Instant::now();
    for i in 0..100_u64 {
        // 16 MiB batches, one every quarter second: exactly 64 MiB/s.
        let at = start + Duration::from_millis(250 * i);
        assert_eq!(rate.reserve(at, 16 * MIB), Duration::ZERO, "batch {i}");
    }
}

// docs/specs/reader-warm-copy AC-10
#[test]
fn an_idle_peer_banks_no_credit() {
    let rate = ServeRate::new(MIB);
    let start = Instant::now();
    assert_eq!(rate.reserve(start, MIB), Duration::ZERO);
    let later = start + Duration::from_secs(3600);
    assert_eq!(rate.reserve(later, 4 * MIB), Duration::ZERO);
    assert_eq!(
        rate.reserve(later, MIB),
        Duration::from_secs(4),
        "not an hour of credit"
    );
}
