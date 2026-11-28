//! The circuit breaker (docs/specs/plugin-system AC-10).

use oss_ext::breaker::{Breaker, BreakerConfig, BreakerState, Decision, Outcome};

fn plain() -> BreakerConfig {
    BreakerConfig {
        jitter_permille: 0,
        ..BreakerConfig::default()
    }
}

fn fail(b: &mut Breaker, now: u64) {
    let d = b.allow(now);
    assert_eq!(d, Decision::Call, "at {now}");
    b.record(d, Outcome::Failure, now);
}

fn ok(b: &mut Breaker, now: u64) {
    let d = b.allow(now);
    assert_eq!(d, Decision::Call, "at {now}");
    b.record(d, Outcome::Success, now);
}

#[test]
fn the_defaults_are_the_adrs() {
    let c = BreakerConfig::default();
    assert_eq!(
        (
            c.consecutive,
            c.window,
            c.min_calls,
            c.open_ms,
            c.max_open_ms,
            c.jitter_permille
        ),
        (5, 20, 10, 30_000, 300_000, 200)
    );
}

#[test]
fn five_consecutive_failures_open_it_and_four_do_not() {
    let mut b = Breaker::new(plain(), 1);
    for _ in 0..4 {
        fail(&mut b, 0);
    }
    assert_eq!(b.state(), BreakerState::Closed);
    ok(&mut b, 0);
    for _ in 0..4 {
        fail(&mut b, 0);
    }
    assert_eq!(b.state(), BreakerState::Closed, "a success resets the run");
    fail(&mut b, 0);
    assert_eq!(b.state(), BreakerState::Open);
}

#[test]
fn half_of_the_last_twenty_open_it_once_ten_are_seen() {
    let mut b = Breaker::new(plain(), 1);
    // Alternating never reaches five in a row; ten calls, five failures: half.
    for i in 0..9 {
        if i % 2 == 0 {
            fail(&mut b, 0);
        } else {
            ok(&mut b, 0);
        }
    }
    assert_eq!(b.state(), BreakerState::Closed, "nine calls are too few");
    ok(&mut b, 0); // the tenth: failures are 5 of 10? i=0,2,4,6,8 failed
    assert_eq!(b.state(), BreakerState::Open);
}

#[test]
fn fewer_than_half_stay_closed_and_old_calls_age_out_of_the_window() {
    let mut b = Breaker::new(plain(), 1);
    for i in 0..60 {
        if i % 3 == 0 {
            fail(&mut b, 0);
        } else {
            ok(&mut b, 0);
        }
    }
    assert_eq!(b.state(), BreakerState::Closed);
}

#[test]
fn open_rejects_without_calling_until_the_time_passes() {
    let mut b = Breaker::new(plain(), 1);
    for _ in 0..5 {
        fail(&mut b, 1_000);
    }
    for t in [1_000, 15_000, 30_999] {
        assert_eq!(b.allow(t), Decision::Reject, "at {t}");
    }
    assert_eq!(b.state(), BreakerState::Open);
    assert_eq!(b.allow(31_000), Decision::Probe);
    assert_eq!(b.state(), BreakerState::HalfOpen);
}

#[test]
fn half_open_admits_one_probe_at_a_time() {
    let mut b = Breaker::new(plain(), 1);
    for _ in 0..5 {
        fail(&mut b, 0);
    }
    let probe = b.allow(30_000);
    assert_eq!(probe, Decision::Probe);
    assert_eq!(b.allow(30_001), Decision::Reject);
    assert_eq!(b.allow(30_002), Decision::Reject);
    b.record(probe, Outcome::Success, 30_003);
    assert_eq!(b.state(), BreakerState::Closed);
    assert_eq!(b.allow(30_004), Decision::Call);
}

#[test]
fn a_failed_probe_reopens_with_the_time_doubled_up_to_five_minutes() {
    let mut b = Breaker::new(plain(), 1);
    for _ in 0..5 {
        fail(&mut b, 0);
    }
    let mut now = 30_000;
    for wait in [60_000_u64, 120_000, 240_000, 300_000, 300_000] {
        let p = b.allow(now);
        assert_eq!(p, Decision::Probe);
        b.record(p, Outcome::Failure, now);
        assert_eq!(b.state(), BreakerState::Open);
        assert_eq!(b.allow(now + wait - 1), Decision::Reject, "wait {wait}");
        now += wait;
        assert_eq!(b.allow(now - 1), Decision::Reject);
    }
    // A success closes it and the next trip starts again at 30 s.
    let p = b.allow(now);
    b.record(p, Outcome::Success, now);
    for _ in 0..5 {
        fail(&mut b, now);
    }
    assert_eq!(b.allow(now + 29_999), Decision::Reject);
    assert_eq!(b.allow(now + 30_000), Decision::Probe);
}

#[test]
fn a_neutral_outcome_counts_for_nothing() {
    let mut b = Breaker::new(plain(), 1);
    for _ in 0..4 {
        fail(&mut b, 0);
    }
    for _ in 0..100 {
        let d = b.allow(0);
        b.record(d, Outcome::Neutral, 0);
    }
    assert_eq!(b.state(), BreakerState::Closed);
    fail(&mut b, 0);
    assert_eq!(
        b.state(),
        BreakerState::Open,
        "the run of four was not reset"
    );
}

#[test]
fn a_neutral_probe_is_released_and_the_next_call_probes() {
    let mut b = Breaker::new(plain(), 1);
    for _ in 0..5 {
        fail(&mut b, 0);
    }
    let p = b.allow(30_000);
    b.record(p, Outcome::Neutral, 30_000);
    assert_eq!(b.state(), BreakerState::HalfOpen);
    assert_eq!(b.allow(30_001), Decision::Probe);
}

#[test]
fn a_call_that_finishes_after_the_trip_changes_nothing() {
    let mut b = Breaker::new(plain(), 1);
    let slow = b.allow(0);
    for _ in 0..5 {
        fail(&mut b, 0);
    }
    b.record(slow, Outcome::Success, 1);
    assert_eq!(
        b.state(),
        BreakerState::Open,
        "a late success does not close it"
    );
    assert_eq!(b.allow(2), Decision::Reject);
}

#[test]
fn the_open_time_is_jittered_within_twenty_percent_and_by_seed() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..200_u64 {
        let mut b = Breaker::new(BreakerConfig::default(), seed);
        for _ in 0..5 {
            fail(&mut b, 0);
        }
        let mut t = 24_000;
        while b.allow(t) == Decision::Reject {
            t += 1;
        }
        assert!(
            (24_000..=36_000).contains(&t),
            "seed {seed}: reopened at {t}"
        );
        seen.insert(t);
    }
    assert!(seen.len() > 20, "jitter varies by seed");
    let a = Breaker::new(BreakerConfig::default(), 7);
    let b = Breaker::new(BreakerConfig::default(), 7);
    assert_eq!(a.state(), b.state());
}
