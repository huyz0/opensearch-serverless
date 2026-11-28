use super::*;

const MIB: u64 = 1 << 20;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// One second of `n` searches of `took` each, with a copy batch in it when
/// `serving`, starting at `at`; returns the instant after.
fn second(p: &Pressure, at: Instant, took: Duration, serving: bool) -> Instant {
    for i in 0..10u32 {
        p.record(at + Duration::from_millis(u64::from(i) * 90), took);
    }
    if serving {
        let _ = p.rate_for(at + ms(950), 1);
    }
    at + Duration::from_millis(1_050)
}

// docs/specs/reader-warm-copy AC-13
#[test]
fn a_peer_starts_at_a_quarter_of_its_ceiling() {
    let t = Instant::now();
    let p = Pressure::new(64 * MIB, t);
    assert_eq!(p.rate_for(t, 0), 16 * MIB);
}

// docs/specs/reader-warm-copy AC-13
#[test]
fn quiet_searches_let_the_rate_climb_to_the_ceiling_and_no_further() {
    let mut t = Instant::now();
    let p = Pressure::new(64 * MIB, t);
    for _ in 0..3 {
        t = second(&p, t, ms(50), false);
    }
    for _ in 0..10 {
        t = second(&p, t, ms(50), true);
    }
    t = second(&p, t, ms(50), true);
    assert_eq!(p.rate_for(t, 1), 64 * MIB);
}

// docs/specs/reader-warm-copy AC-13
#[test]
fn slowed_searches_while_serving_halve_the_rate_down_to_the_floor() {
    let mut t = Instant::now();
    let p = Pressure::new(64 * MIB, t);
    for _ in 0..4 {
        t = second(&p, t, ms(50), false);
    }
    for _ in 0..6 {
        t = second(&p, t, ms(50), true);
    }
    let high = p.rate_for(t, 1);
    for _ in 0..12 {
        t = second(&p, t, ms(120), true);
    }
    let low = p.rate_for(t, 1);
    assert!(low < high, "{low} is below {high}");
    assert_eq!(low, 8 * MIB, "the floor is an eighth of the ceiling");
}

// docs/specs/reader-warm-copy AC-13
#[test]
fn without_a_quiet_baseline_slow_searches_do_not_throttle() {
    let mut t = Instant::now();
    let p = Pressure::new(64 * MIB, t);
    for _ in 0..8 {
        t = second(&p, t, ms(500), true);
    }
    assert_eq!(p.rate_for(t, 1), 64 * MIB, "nothing to compare with");
}

// docs/specs/reader-warm-copy AC-13
#[test]
fn a_window_with_few_searches_says_nothing() {
    let mut t = Instant::now();
    let p = Pressure::new(64 * MIB, t);
    for _ in 0..3 {
        t = second(&p, t, ms(50), false);
    }
    for _ in 0..5 {
        p.record(t, ms(900));
        let _ = p.rate_for(t, 1);
        t += ms(1_050);
    }
    assert!(p.rate_for(t, 1) >= 16 * MIB, "no backing off on one search");
}
