//! docs/specs/coordinator-search-top-hits: a search merges a top-level
//! `top_hits` across its shards and writes its hits from the shards that
//! ranked them.

#![allow(clippy::arithmetic_side_effects)] // ARITH: test counters over a few shards.

use std::sync::Mutex;

use oss_coordinator::reduce::aggs::AggsError;
use oss_coordinator::search::{Params, SearchFailure, Shards, TopHitsWindow, search};
use oss_reader::aggs::metric::top_hits::TopHitsResult;
use oss_reader::aggs::partial::{Format, Partial, Value};
use oss_reader::fetch::source_filter::SourceFilter;
use oss_reader::search::{Hit, Hits};

struct Fake {
    shards: Vec<Hits>,
    window: Option<TopHitsWindow>,
    asked: Mutex<Vec<(usize, String, Vec<usize>)>>,
}

fn top(scores: &[f32]) -> TopHitsResult {
    let mut result = TopHitsResult::empty();
    result.total = scores.len() as u64;
    result.max_score = scores.first().copied();
    result.hits = scores
        .iter()
        .enumerate()
        .map(|(i, s)| Hit {
            doc: i as i32,
            score: Some(*s),
            sort: Vec::new(),
        })
        .collect();
    result
}

fn shard(scores: &[f32]) -> Hits {
    Hits {
        total: 0,
        exact: true,
        max_score: None,
        hits: Vec::new(),
        aggregations: vec![Partial {
            name: "best".into(),
            format: Format::Raw,
            value: Value::TopHits(Box::new(top(scores))),
        }],
        tracked: true,
        source: SourceFilter::default(),
        fields: Vec::new(),
        explanations: Vec::new(),
        highlight: None,
        meta: oss_reader::fetch::hit_meta::HitMeta::default(),
        terminated_early: None,
    }
}

impl Shards for Fake {
    type Query = Hits;
    type Error = String;

    fn index(&self, _: usize) -> &str {
        "idx"
    }

    fn query(&self, shard: usize) -> Result<Hits, String> {
        Ok(self.shards[shard].clone())
    }

    fn hits<'a>(&self, query: &'a Hits) -> &'a Hits {
        query
    }

    fn render(&self, _: usize, _: &Hits, positions: &[usize]) -> Result<Vec<Vec<u8>>, String> {
        Ok(positions.iter().map(|_| b"{}".to_vec()).collect())
    }

    fn top_hits_window(&self, name: &str) -> Option<TopHitsWindow> {
        if name == "best" {
            self.window.clone()
        } else {
            None
        }
    }

    fn render_top_hits(
        &self,
        shard: usize,
        _: &Hits,
        name: &str,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, String> {
        self.asked
            .lock()
            .expect("lock")
            .push((shard, name.to_owned(), positions.to_vec()));
        Ok(positions
            .iter()
            .map(|p| format!("{{\"h\":\"s{shard}p{p}\"}}").into_bytes())
            .collect())
    }
}

fn fake(window: Option<(usize, usize)>) -> Fake {
    Fake {
        shards: vec![shard(&[3.0, 1.0]), shard(&[2.0, 1.5])],
        window: window.map(|(from, size)| TopHitsWindow {
            from,
            size,
            sorts: None,
        }),
        asked: Mutex::new(Vec::new()),
    }
}

fn params() -> Params {
    Params {
        from: 0,
        size: 0,
        sorts: None,
        track_total_hits_up_to: Some(10_000),
        show_total: true,
        concurrency: 2,
        took_ms: 1,
    }
}

#[test]
fn the_aggregation_holds_the_merged_page_from_the_shards_that_ranked_it() {
    let f = fake(Some((0, 3)));
    let body = String::from_utf8(search(&f, 2, &params()).expect("searches")).expect("utf-8");
    assert!(
        body.contains(
            r#""aggregations":{"best":{"hits":{"total":{"value":4,"relation":"eq"},"max_score":3.0,"hits":[{"h":"s0p0"},{"h":"s1p0"},{"h":"s1p1"}]}}}"#
        ),
        "{body}"
    );
    let mut asked = f.asked.lock().expect("lock").clone();
    asked.sort();
    assert_eq!(
        asked,
        [
            (0, "best".to_owned(), vec![0]),
            (1, "best".to_owned(), vec![0, 1])
        ]
    );
}

#[test]
fn from_is_applied_once_over_all_the_shards() {
    let f = fake(Some((2, 2)));
    let body = String::from_utf8(search(&f, 2, &params()).expect("searches")).expect("utf-8");
    assert!(
        body.contains(r#""hits":[{"h":"s1p1"},{"h":"s0p1"}]}}}"#),
        "{body}"
    );
}

#[test]
fn a_top_hits_the_request_does_not_describe_is_refused_as_before() {
    let f = fake(None);
    assert_eq!(
        search(&f, 2, &params()).expect_err("refused"),
        SearchFailure::Aggs(AggsError::TopHits {
            name: "best".into()
        })
    );
}
