//! docs/specs/coordinator-search: one search over a fake set of shards.

#![allow(clippy::arithmetic_side_effects)] // ARITH: test counters over a few shards.

use std::sync::Mutex;

use oss_coordinator::search::{Params, SearchFailure, Shards, search};
use oss_reader::aggs::partial::{Format, Partial, Value};
use oss_reader::fetch::source_filter::SourceFilter;
use oss_reader::search::{Hit, Hits};

struct Fake {
    shards: Vec<Result<Hits, String>>,
    renders: Mutex<Vec<(usize, Vec<usize>)>>,
    fail_render: Option<usize>,
}

fn hits(total: u64, scored: &[f32], aggs: Vec<Partial>) -> Hits {
    Hits {
        total,
        exact: true,
        max_score: scored.first().copied(),
        hits: scored
            .iter()
            .enumerate()
            .map(|(i, s)| Hit {
                doc: i as i32,
                score: Some(*s),
                sort: Vec::new(),
            })
            .collect(),
        aggregations: aggs,
        tracked: true,
        total_as_int: false,
        typed_keys: false,
        source: SourceFilter::default(),
        fields: Vec::new(),
        explanations: Vec::new(),
        highlight: None,
        meta: oss_reader::fetch::hit_meta::HitMeta::default(),
        inner: Vec::new(),
        terminated_early: None,
    }
}

fn fake(shards: Vec<Result<Hits, String>>) -> Fake {
    Fake {
        shards,
        renders: Mutex::new(Vec::new()),
        fail_render: None,
    }
}

impl Shards for Fake {
    type Query = Hits;
    type Error = String;

    fn index(&self, shard: usize) -> &str {
        let _ = shard;
        "idx"
    }

    fn query(&self, shard: usize) -> Result<Hits, String> {
        self.shards[shard].clone()
    }

    fn hits<'a>(&self, query: &'a Hits) -> &'a Hits {
        query
    }

    fn render(
        &self,
        shard: usize,
        _query: &Hits,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, String> {
        self.renders
            .lock()
            .expect("lock")
            .push((shard, positions.to_vec()));
        if self.fail_render == Some(shard) {
            return Err("fetch broke".into());
        }
        Ok(positions
            .iter()
            .map(|p| format!("{{\"h\":\"s{shard}p{p}\"}}").into_bytes())
            .collect())
    }

    fn render_top_hits(
        &self,
        _shard: usize,
        _query: &Hits,
        _result: &oss_reader::aggs::metric::top_hits::TopHitsResult,
        _positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, String> {
        Err("no top_hits in this fake".into())
    }
}

fn params(from: usize, size: usize) -> Params {
    Params {
        from,
        size,
        sorts: None,
        track_total_hits_up_to: Some(10_000),
        show_total: true,
        total_as_int: false,
        typed_keys: false,
        concurrency: 4,
        took_ms: 7,
    }
}

fn text(body: Vec<u8>) -> String {
    String::from_utf8(body).expect("utf8")
}

#[test]
fn the_page_is_the_merged_order_of_the_shards_hits() {
    let f = fake(vec![
        Ok(hits(3, &[3.0, 1.0, 0.5], vec![])),
        Ok(hits(2, &[2.0, 1.5], vec![])),
    ]);
    let body = text(search(&f, 2, &params(1, 3)).expect("searches"));
    assert!(
        body.contains(r#""hits":[{"h":"s1p0"},{"h":"s1p1"},{"h":"s0p1"}]"#),
        "{body}"
    );
}

#[test]
fn a_shard_renders_once_and_only_when_the_page_draws_on_it() {
    let f = fake(vec![
        Ok(hits(3, &[3.0, 2.5, 0.5], vec![])),
        Ok(hits(1, &[0.1], vec![])),
    ]);
    search(&f, 2, &params(0, 2)).expect("searches");
    let calls = f.renders.lock().expect("lock").clone();
    assert_eq!(calls, vec![(0, vec![0, 1])]);
}

#[test]
fn the_response_envelope_is_written_exactly() {
    let f = fake(vec![Ok(hits(1, &[2.5], vec![]))]);
    assert_eq!(
        text(search(&f, 1, &params(0, 10)).expect("searches")),
        r#"{"took":7,"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":1,"relation":"eq"},"max_score":2.5,"hits":[{"h":"s0p0"}]}}"#
    );
    let mut capped = params(0, 10);
    capped.track_total_hits_up_to = Some(1);
    let mut inexact = hits(5, &[2.5], vec![]);
    inexact.exact = false;
    let f = fake(vec![Ok(inexact)]);
    let body = text(search(&f, 1, &capped).expect("searches"));
    assert!(
        body.contains(r#""total":{"value":1,"relation":"gte"}"#),
        "{body}"
    );
    let f = fake(vec![Ok(hits(0, &[], vec![]))]);
    let body = text(search(&f, 1, &params(0, 10)).expect("searches"));
    assert!(body.contains(r#""max_score":null,"hits":[]"#), "{body}");
}

#[test]
fn terminated_early_follows_timed_out_when_a_shard_says_and_is_or_of_the_shards() {
    let said = |early: Option<bool>| {
        let mut h = hits(1, &[2.5], vec![]);
        h.terminated_early = early;
        Ok(h)
    };
    let body = |shards: Vec<Result<Hits, String>>| {
        let n = shards.len();
        text(search(&fake(shards), n, &params(0, 10)).expect("searches"))
    };
    let head = |early: &str, n: usize| {
        format!(
            r#"{{"took":7,"timed_out":false,"terminated_early":{early},"_shards":{{"total":{n},"#
        )
    };
    assert!(body(vec![said(Some(true))]).starts_with(&head("true", 1)));
    assert!(body(vec![said(Some(false))]).starts_with(&head("false", 1)));
    assert!(body(vec![said(Some(false)), said(Some(true))]).starts_with(&head("true", 2)));
    assert!(body(vec![said(None), said(Some(false))]).starts_with(&head("false", 2)));
    // No shard says: no key.
    assert!(!body(vec![said(None), said(None)]).contains("terminated_early"));
}

#[test]
fn an_untracked_total_is_not_written() {
    let f = fake(vec![Ok(hits(1, &[2.5], vec![]))]);
    let mut p = params(0, 10);
    p.show_total = false;
    let body = text(search(&f, 1, &p).expect("searches"));
    assert!(
        body.contains(r#""hits":{"max_score":2.5,"hits":["#),
        "{body}"
    );
}

#[test]
fn a_failed_shard_is_counted_and_listed() {
    let f = fake(vec![Ok(hits(1, &[2.5], vec![])), Err("no \"copy\"".into())]);
    let body = text(search(&f, 2, &params(0, 10)).expect("searches"));
    assert!(
        body.contains(
            r#""_shards":{"total":2,"successful":1,"skipped":0,"failed":1,"failures":[{"shard":1,"index":"idx","reason":{"type":"exception","reason":"no \"copy\""}}]}"#
        ),
        "{body}"
    );
    assert!(body.contains(r#""hits":[{"h":"s0p0"}]"#), "{body}");
}

#[test]
fn every_shard_failing_fails_the_search() {
    let f = fake(vec![Err("first".into()), Err("second".into())]);
    match search(&f, 2, &params(0, 10)) {
        Err(SearchFailure::AllFailed { error }) => assert_eq!(error, "first"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_failed_render_fails_the_search() {
    let mut f = fake(vec![Ok(hits(1, &[2.5], vec![]))]);
    f.fail_render = Some(0);
    match search(&f, 1, &params(0, 10)) {
        Err(SearchFailure::Fetch { shard, reason }) => {
            assert_eq!(shard, 0);
            assert_eq!(reason, "fetch broke");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn aggregations_are_reduced_and_written() {
    let sum = |v| Partial {
        name: "total".into(),
        format: Format::Raw,
        value: Value::Sum(v),
    };
    let f = fake(vec![
        Ok(hits(1, &[1.0], vec![sum(1.5)])),
        Ok(hits(1, &[0.5], vec![sum(2.0)])),
    ]);
    let body = text(search(&f, 2, &params(0, 10)).expect("searches"));
    assert!(
        body.contains(r#""aggregations":{"total":{"value":3.5}}"#),
        "{body}"
    );
    let f = fake(vec![Ok(hits(1, &[1.0], vec![]))]);
    assert!(!text(search(&f, 1, &params(0, 10)).expect("ok")).contains("aggregations"));
}

#[test]
fn no_shards_is_an_empty_response() {
    let f = fake(vec![]);
    let body = text(search(&f, 0, &params(0, 10)).expect("searches"));
    assert!(
        body.contains(r#""_shards":{"total":0,"successful":0,"skipped":0,"failed":0}"#),
        "{body}"
    );
}

#[test]
fn several_failed_shards_are_listed_in_order() {
    let f = fake(vec![
        Err("a".into()),
        Ok(hits(1, &[1.0], vec![])),
        Err("b".into()),
    ]);
    let body = text(search(&f, 3, &params(0, 10)).expect("searches"));
    assert!(
        body.contains(
            r#""failures":[{"shard":0,"index":"idx","reason":{"type":"exception","reason":"a"}},{"shard":2,"index":"idx","reason":{"type":"exception","reason":"b"}}]"#
        ),
        "{body}"
    );
}

#[test]
fn an_aggregation_keeps_the_format_its_shards_gave() {
    let max = |v| Partial {
        name: "latest".into(),
        format: Format::Date,
        value: Value::Max(v),
    };
    let f = fake(vec![
        Ok(hits(1, &[1.0], vec![max(1.0e12)])),
        Ok(hits(1, &[0.5], vec![max(2.0e12)])),
    ]);
    let body = text(search(&f, 2, &params(0, 10)).expect("searches"));
    assert!(
        body.contains(r#""latest":{"value":2.0E12,"value_as_string":"#),
        "{body}"
    );
}
