//! The limiter's boundaries: each comparison at the value where it flips.

use super::*;

const MS: u64 = MILLI;

#[test]
fn a_refusal_displays_its_reason() {
    let t = Throttled {
        retry_after_secs: 2,
        // ALLOC: test data, a few bytes.
        reason: "slow store".to_owned(),
    };
    // ALLOC: test data, a few bytes.
    assert_eq!(t.to_string(), "slow store");
}

#[test]
fn a_second_cut_exactly_500ms_after_the_first_is_not_made() {
    let l = WriteLimiter::new(100, 500, 2, 10);
    l.on_append(0, 200 * MS, true);
    assert_eq!(l.stats().limit, 6);
    l.on_append(500 * MS, 200 * MS, true);
    assert_eq!(l.stats().limit, 6, "500 ms is not more than 500 ms");
    l.on_append(500 * MS + 1, 200 * MS, true);
    assert_eq!(l.stats().limit, 4);
}

#[test]
fn a_write_waiting_exactly_the_target_does_not_cut_the_limit() {
    let l = WriteLimiter::new(100, 500, 2, 10);
    let _stuck = l.admit(0).expect("admitted");
    let _next = l.admit(100 * MS).expect("admitted");
    assert_eq!(l.stats().limit, 10);
    let _last = l.admit(100 * MS + 1).expect("admitted");
    assert_eq!(l.stats().limit, 6, "one nanosecond more is a cut");
}

#[test]
fn pressure_exactly_the_budget_still_admits_a_write_beyond_the_minimum() {
    let l = WriteLimiter::new(100, 500, 2, 10);
    let _a = l.admit(0).expect("admitted");
    let _b = l.admit(0).expect("admitted");
    let _c = l
        .admit(500 * MS)
        .expect("the oldest waited the budget, not more");
    assert!(l.admit(500 * MS + SECOND).is_err(), "past it is refused");
}

#[test]
fn a_smoothed_append_exactly_the_target_is_not_a_cut() {
    let l = WriteLimiter::new(100, 500, 2, 10);
    l.on_append(0, 100 * MS, true);
    assert_eq!(l.stats().limit, 10);
    let l = WriteLimiter::new(100, 500, 2, 10);
    l.on_append(0, 100 * MS + 1, true);
    assert_eq!(l.stats().limit, 6);
}

/// A limiter of maximum 1,000 cut once to 666.67, with no append sampled.
fn cut_limiter() -> WriteLimiter {
    let l = WriteLimiter::new(100, 500, 2, 1_000);
    let a = l.admit(0).expect("admitted");
    let b = l.admit(150 * MS).expect("admitted");
    drop((a, b));
    assert_eq!(l.stats().limit, 666);
    l
}

#[test]
fn a_fast_append_grows_the_limit_geometrically() {
    let l = cut_limiter();
    // 666.67 * 1.02 + 0.5 = 680.67.
    l.on_append(SECOND, 10 * MS, true);
    assert_eq!(l.stats().limit, 680);
}

#[test]
fn an_append_of_exactly_half_the_target_grows_by_the_additive_step() {
    let l = cut_limiter();
    l.on_append(SECOND, 50 * MS, true);
    assert_eq!(l.stats().limit, 666, "4 / 666 is a hundredth of a slot");
}
