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
        result: &TopHitsResult,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, String> {
        let _ = result;
        self.asked
            .lock()
            .expect("lock")
            .push((shard, "best".to_owned(), positions.to_vec()));
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

mod under_a_bucket {
    use super::*;
    use oss_reader::aggs::bucket::keyword_terms::KeywordTermsResult;
    use oss_reader::aggs::bucket::terms::TermsSpec;

    /// A shard whose `t` terms holds the bucket `key`, with a `top_hits`
    /// named `best` of `scores` in it.
    fn bucketed(key: &str, scores: &[f32]) -> Hits {
        let mut hits = shard(&[]);
        hits.aggregations = vec![Partial {
            name: "t".into(),
            format: Format::Raw,
            value: Value::KeywordTerms(Box::new(KeywordTermsResult {
                spec: TermsSpec::default(),
                other_doc_count: 0,
                doc_count_error: 0,
                buckets: vec![(key.to_owned(), scores.len() as u64)],
                bucket_errors: Vec::new(),
                sub_meta: vec![("best".to_owned(), Format::Raw)],
                subs: vec![vec![Value::TopHits(Box::new(top(scores)))]],
            })),
        }];
        hits
    }

    #[test]
    fn a_bucket_two_shards_hold_has_its_hits_merged_and_written_by_their_shards() {
        let f = Fake {
            shards: vec![bucketed("a", &[3.0, 1.0]), bucketed("a", &[2.0, 1.5])],
            window: Some(TopHitsWindow {
                from: 0,
                size: 3,
                sorts: None,
            }),
            asked: Mutex::new(Vec::new()),
        };
        let body = String::from_utf8(search(&f, 2, &params()).expect("searches")).expect("utf-8");
        assert!(
            body.contains(
                r#""best":{"hits":{"total":{"value":4,"relation":"eq"},"max_score":3.0,"hits":[{"h":"s0p0"},{"h":"s1p0"},{"h":"s1p1"}]}}"#
            ),
            "{body}"
        );
    }

    #[test]
    fn a_bucket_one_shard_holds_is_written_from_that_shard() {
        let f = Fake {
            shards: vec![bucketed("a", &[3.0]), bucketed("b", &[2.0])],
            window: Some(TopHitsWindow {
                from: 0,
                size: 2,
                sorts: None,
            }),
            asked: Mutex::new(Vec::new()),
        };
        let body = String::from_utf8(search(&f, 2, &params()).expect("searches")).expect("utf-8");
        assert!(
            body.contains(r#"{"h":"s0p0"}"#) && body.contains(r#"{"h":"s1p0"}"#),
            "{body}"
        );
        let mut asked = f.asked.lock().expect("lock").clone();
        asked.sort();
        assert_eq!(
            asked.iter().map(|a| a.0).collect::<Vec<_>>(),
            [0, 1],
            "each shard is asked once, for its own bucket"
        );
    }
}
