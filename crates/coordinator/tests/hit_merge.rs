//! docs/specs/coordinator-hit-merge: the hits of every shard become one page.

// ARITH: test counts and scores are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::reduce::hits::{
    ACCURATE, MergeError, Merger, Relation, ShardAnswer, ShardHit, SortSpec,
};
use oss_reader::search::SortValue;

fn hit(payload: &'static str, score: Option<f32>, sort: Vec<SortValue>) -> ShardHit<&'static str> {
    ShardHit {
        score,
        sort,
        payload,
    }
}

fn scored(payload: &'static str, score: f32) -> ShardHit<&'static str> {
    hit(payload, Some(score), Vec::new())
}

fn answer(hits: Vec<ShardHit<&'static str>>) -> ShardAnswer<&'static str> {
    ShardAnswer {
        total: u64::try_from(hits.len()).expect("small"),
        exact: true,
        max_score: None,
        hits,
        timed_out: false,
        terminated_early: None,
    }
}

fn page(merger: Merger<&'static str>) -> Vec<&'static str> {
    merger
        .finish(None)
        .expect("merges")
        .page
        .iter()
        .map(|h| h.payload)
        .collect()
}

fn asc() -> SortSpec {
    SortSpec {
        descending: false,
        missing_first: false,
    }
}

#[test]
fn no_sort_orders_by_score_stably_with_unscored_hits_last() {
    let mut m = Merger::new(0, 10, None);
    m.absorb(answer(vec![
        scored("a", 1.0),
        scored("b", 3.0),
        hit("n", None, vec![]),
    ]))
    .expect("absorbed");
    m.absorb(answer(vec![scored("c", 3.0), scored("d", 2.0)]))
        .expect("absorbed");
    assert_eq!(page(m), ["b", "c", "d", "a", "n"]);
}

#[test]
fn field_sorts_run_in_their_direction_with_missing_values_placed_by_the_sort() {
    let long = |n| vec![SortValue::Long(n)];
    let term = |t: Option<&str>| vec![SortValue::Term(t.map(|s| s.as_bytes().to_vec()))];
    let hits = |ids: [(&'static str, i64); 3]| {
        answer(ids.iter().map(|(p, n)| hit(p, None, long(*n))).collect())
    };
    let mut m = Merger::new(0, 10, Some(vec![asc()]));
    m.absorb(hits([("a", 3), ("b", 1), ("c", 2)]))
        .expect("absorbed");
    assert_eq!(page(m), ["b", "c", "a"]);
    let mut m = Merger::new(
        0,
        10,
        Some(vec![SortSpec {
            descending: true,
            missing_first: false,
        }]),
    );
    m.absorb(hits([("a", 3), ("b", 1), ("c", 2)]))
        .expect("absorbed");
    assert_eq!(page(m), ["a", "c", "b"]);
    // Missing keyword values: last whichever the direction, unless _first.
    for (descending, missing_first, expect) in [
        (false, false, ["x", "y", "m"]),
        (true, false, ["y", "x", "m"]),
        (false, true, ["m", "x", "y"]),
        (true, true, ["m", "y", "x"]),
    ] {
        let mut m = Merger::new(
            0,
            10,
            Some(vec![SortSpec {
                descending,
                missing_first,
            }]),
        );
        m.absorb(answer(vec![
            hit("m", None, term(None)),
            hit("y", None, term(Some("b"))),
            hit("x", None, term(Some("a"))),
        ]))
        .expect("absorbed");
        assert_eq!(
            page(m),
            expect,
            "descending {descending} first {missing_first}"
        );
    }
    // A second key breaks a tie; a hit with no sort values goes last.
    let mut m = Merger::new(0, 10, Some(vec![asc(), asc()]));
    m.absorb(answer(vec![
        hit("nosort", None, vec![]),
        hit("p", None, vec![SortValue::Long(1), SortValue::Long(9)]),
        hit("q", None, vec![SortValue::Long(1), SortValue::Long(2)]),
    ]))
    .expect("absorbed");
    assert_eq!(page(m), ["q", "p", "nosort"]);
}

#[test]
fn sort_values_of_different_kinds_are_refused_when_compared() {
    let mut m = Merger::new(0, 10, Some(vec![asc()]));
    m.absorb(answer(vec![
        hit("a", None, vec![SortValue::Long(1)]),
        hit("b", None, vec![SortValue::Term(Some(b"x".to_vec()))]),
    ]))
    .expect("absorbed");
    let err = m.finish(None).expect_err("refused");
    assert_eq!(err, MergeError::IncompatibleSortValues);
    assert_eq!(
        err.to_string(),
        "cannot merge incompatible sort values across indices; use consistent field mappings \
         or numeric_type for numeric sorts"
    );
}

#[test]
fn totals_relation_best_score_and_flags_combine() {
    let mut m: Merger<&'static str> = Merger::new(0, 10, None);
    let mut a = answer(vec![scored("a", 1.0)]);
    a.total = 7;
    a.max_score = Some(1.0);
    let mut b = answer(vec![scored("b", 2.0)]);
    b.total = 5;
    b.exact = false;
    b.max_score = Some(2.5);
    b.timed_out = true;
    b.terminated_early = Some(false);
    let mut c = answer(vec![]);
    c.total = 1;
    c.terminated_early = Some(true);
    for x in [a, b, c] {
        m.absorb(x).expect("absorbed");
    }
    let merged = m.finish(None).expect("merges");
    assert_eq!(merged.total, 13);
    assert_eq!(merged.relation, Relation::AtLeast);
    assert_eq!(merged.max_score, Some(2.5));
    assert!(merged.timed_out);
    assert_eq!(merged.terminated_early, Some(true));
    assert_eq!(merged.answered, 3);
    let mut none: Merger<&'static str> = Merger::new(0, 10, None);
    none.absorb(answer(vec![])).expect("absorbed");
    let merged = none.finish(None).expect("merges");
    assert_eq!(merged.terminated_early, None);
    assert_eq!(merged.relation, Relation::Equal);
    assert_eq!(merged.max_score, None);
    assert!(!merged.timed_out);
    let mut neither: Merger<&'static str> = Merger::new(0, 10, None);
    let mut d = answer(vec![]);
    d.terminated_early = Some(false);
    neither.absorb(d).expect("absorbed");
    assert_eq!(
        neither.finish(None).expect("merges").terminated_early,
        Some(false)
    );
}

#[test]
fn the_pile_stays_within_two_windows_and_the_page_is_unchanged() {
    // 30 shards of 3 hits; the page is the best 2.
    let names: Vec<String> = (0..90).map(|n| format!("h{n}")).collect();
    let leaked: Vec<&'static str> = names
        .iter()
        .map(|s| &*Box::leak(s.clone().into_boxed_str()))
        .collect();
    let build = || {
        let mut m = Merger::new(0, 2, None);
        for shard in 0..30 {
            let hits = (0..3)
                .map(|k| {
                    let n = shard * 3 + k;
                    scored(leaked[n], (n % 17) as f32 + (n as f32) / 1000.0)
                })
                .collect();
            m.absorb(answer(hits)).expect("absorbed");
        }
        m
    };
    let m = build();
    // window 2: the pile is cut once past 4, so it never holds more than 4 + 3.
    assert!(m.peak_held() <= 4 + 3, "peak {}", m.peak_held());
    let cut = page(m);
    let mut all = Merger::new(0, 2, None);
    // Without the cut: one answer holding everything.
    let every = (0..90)
        .map(|n| scored(leaked[n], (n % 17) as f32 + (n as f32) / 1000.0))
        .collect();
    all.absorb(answer(every)).expect("absorbed");
    assert_eq!(cut, page(all));
}

#[test]
fn the_page_is_from_and_size_of_the_sorted_pile() {
    let mut m = Merger::new(1, 2, None);
    m.absorb(answer(vec![
        scored("a", 5.0),
        scored("b", 4.0),
        scored("c", 3.0),
        scored("d", 2.0),
    ]))
    .expect("absorbed");
    assert_eq!(page(m), ["b", "c"]);
    let mut m = Merger::new(3, 5, None);
    m.absorb(answer(vec![scored("a", 2.0), scored("b", 1.0)]))
        .expect("absorbed");
    assert!(page(m).is_empty());
}

#[test]
fn an_at_least_total_is_capped_by_the_tracked_bound() {
    let at_least = |total: u64, up_to: Option<i64>| {
        let mut m: Merger<&'static str> = Merger::new(0, 1, None);
        let mut a = answer(vec![]);
        a.total = total;
        a.exact = false;
        m.absorb(a).expect("absorbed");
        m.finish(up_to).expect("merges").total
    };
    assert_eq!(at_least(50_000, None), 10_000, "the default bound");
    assert_eq!(at_least(50_000, Some(100)), 100);
    assert_eq!(at_least(50, Some(100)), 50);
    assert_eq!(at_least(50_000, Some(0)), 50_000);
    assert_eq!(at_least(50_000, Some(-1)), 50_000);
    assert_eq!(at_least(50_000, Some(ACCURATE)), 50_000);
    let mut m: Merger<&'static str> = Merger::new(0, 1, None);
    let mut a = answer(vec![]);
    a.total = 50_000;
    m.absorb(a).expect("absorbed");
    assert_eq!(m.finish(Some(100)).expect("merges").total, 50_000, "exact");
}
