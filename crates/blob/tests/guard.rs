//! Tenant cost levels (docs/specs/tenant-levels/SPEC.md).

// ARITH: times and counts here are small test constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_blob::guard::{Guardrails, Measure, Mode, Refused, parse_levels};

const TICK: i64 = 10_000;

/// A tenant `t` whose budget is 100 units a minute, given the cost and
/// documents it has added up to.
fn measure(cost_units: u64, docs: u64) -> [Measure<'static>; 1] {
    [Measure {
        tenant: "t",
        cost_milli: cost_units * 1_000,
        docs,
        budget_per_minute: 100,
    }]
}

/// Ticks every ten seconds for `seconds`, the tenant adding `units` of cost
/// and `docs` documents each tick; returns the clock.
fn run(g: &Guardrails, from: i64, seconds: i64, units: u64, docs: u64, at: &mut (u64, u64)) -> i64 {
    let mut now = from;
    while now < from + seconds * 1_000 {
        now += TICK;
        at.0 += units;
        at.1 += docs;
        g.tick(now, &measure(at.0, at.1));
    }
    now
}

// AC-1
#[test]
fn a_tenant_over_budget_climbs_a_level_a_minute_then_two_minutes_apart() {
    let g = Guardrails::new(1_000);
    let mut at = (0, 0);
    g.tick(0, &measure(0, 0));
    // 20 units a tick is 120 a minute: over 100.
    let mut now = run(&g, 0, 50, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 0, "not yet a minute over");
    now = run(&g, now, 20, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 1);
    now = run(&g, now, 110, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 1, "two minutes after the first");
    now = run(&g, now, 20, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 2);
    now = run(&g, now, 130, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 3);
    now = run(&g, now, 600, 20, 1_000, &mut at);
    assert_eq!(
        g.level("t", now),
        3,
        "three is the most a tenant's own cost reaches"
    );
}

// AC-2
#[test]
fn a_tenant_under_budget_stays_at_zero_and_one_that_falls_back_loses_a_level_in_five_minutes() {
    let g = Guardrails::new(1_000);
    let mut at = (0, 0);
    g.tick(0, &measure(0, 0));
    let mut now = run(&g, 0, 80, 5, 10, &mut at);
    assert_eq!(g.level("t", now), 0);
    now = run(&g, now, 80, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 1);
    // Under again: a level comes off only after five minutes.
    now = run(&g, now, 280, 5, 10, &mut at);
    assert_eq!(g.level("t", now), 1);
    now = run(&g, now, 40, 5, 10, &mut at);
    assert_eq!(g.level("t", now), 0);
}

// AC-3
#[test]
fn a_level_set_from_outside_holds_when_higher_and_bad_pairs_are_left_out() {
    let g = Guardrails::new(1_000);
    g.tick(0, &measure(0, 0));
    g.set_levels("t=2, other=9, =3, x=high, solo, d=1");
    assert_eq!(g.level("t", 0), 2);
    assert_eq!(g.level("d", 0), 1);
    assert_eq!(g.level("other", 0), 0);
    let parsed = parse_levels("a=2,b=4,c=5,d=-1,e=x,=1");
    assert_eq!(parsed.len(), 2);
    assert_eq!((parsed["a"], parsed["b"]), (2, 4));
    assert!(parse_levels("").is_empty());
    g.set_levels("");
    assert_eq!(g.level("t", 0), 0);
}

// AC-4
#[test]
fn a_refusal_shows_as_level_four_for_a_minute() {
    let g = Guardrails::new(1_000);
    g.tick(0, &measure(0, 0));
    g.note_refused("t", 5_000);
    assert_eq!(g.level("t", 5_000), 4);
    assert_eq!(g.level("t", 64_999), 4);
    assert_eq!(g.level("t", 65_000), 0);
    assert_eq!(g.report(65_000)[0].1.refused, 1);
    // A tenant never seen has nothing to refuse.
    g.note_refused("ghost", 5_000);
    assert_eq!(g.level("ghost", 5_000), 0);
}

fn at_level_three() -> (Guardrails, i64) {
    let g = Guardrails::new(1_000);
    let mut at = (0, 0);
    g.tick(0, &measure(0, 0));
    let now = run(&g, 0, 330, 20, 1_000, &mut at);
    assert_eq!(g.level("t", now), 3);
    (g, now)
}

// AC-5
#[test]
fn a_write_is_held_to_what_fits_the_budget_and_refused_past_the_longest_wait() {
    let (g, now) = at_level_three();
    // 100 documents a second at 120 units a minute against 100: 83.3 fit.
    assert_eq!(
        g.pace_write(Mode::Enforce, "t", 100, now),
        Ok(0),
        "within the burst"
    );
    // 67 documents short at 83.3 a second: 800 ms, rounded up.
    let held = g.pace_write(Mode::Enforce, "t", 300, now);
    assert!(matches!(held, Ok(800..=801)), "{held:?}");
    // The bucket is now in debt: a write that would wait over a second is refused.
    let refused = g.pace_write(Mode::Enforce, "t", 400, now);
    assert!(matches!(refused, Err(Refused { retry_after_millis }) if retry_after_millis > 1_000));
    assert_eq!(g.level("t", now), 4);
    let report = &g.report(now)[0].1;
    assert_eq!((report.paced, report.refused), (1, 1));
}

// AC-6
#[test]
fn only_an_enforced_tenant_at_level_three_is_paced() {
    let (g, now) = at_level_three();
    for mode in [Mode::Observe, Mode::Off] {
        assert_eq!(g.pace_write(mode, "t", 1_000_000, now), Ok(0));
    }
    assert_eq!(g.pace_write(Mode::Enforce, "t", 0, now), Ok(0));
    assert_eq!(g.pace_write(Mode::Enforce, "nobody", 1_000_000, now), Ok(0));
    assert!(g.enforced_at(Mode::Enforce, "t", 3, now));
    assert!(!g.enforced_at(Mode::Observe, "t", 1, now));
    // Back under budget the tenant is not paced even at its level.
    let mut at = (0, 0);
    at.0 = 20 * 33;
    g.tick(now + TICK, &measure(at.0 + 1, 33_000 + 1));
    assert_eq!(
        g.pace_write(Mode::Enforce, "t", 1_000_000, now + TICK),
        Ok(0)
    );
}

// AC-7
#[test]
fn a_tenant_a_tick_does_not_name_is_forgotten() {
    let g = Guardrails::new(1_000);
    g.tick(0, &measure(0, 0));
    g.set_levels("t=2");
    g.tick(TICK, &[]);
    assert!(g.report(TICK).is_empty());
    assert_eq!(
        g.level("t", TICK),
        2,
        "a level set from outside outlives it"
    );
    assert_eq!(Mode::parse("enforce"), Some(Mode::Enforce));
    assert_eq!(Mode::parse("observe"), Some(Mode::Observe));
    assert_eq!(Mode::parse("off"), Some(Mode::Off));
    assert_eq!(Mode::parse("loud"), None);
}
