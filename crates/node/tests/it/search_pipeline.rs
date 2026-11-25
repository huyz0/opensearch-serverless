//! docs/specs/search-pipelines: `?search_pipeline=` over `_search`, end to end
//! against pipelines held in memory (the stored set is in the server tests).

use std::collections::HashMap;
use std::sync::Arc;

use oss_node::search::{PipelineFault, PipelineSource};
use oss_reader::search_pipeline::SearchPipeline;

use crate::support::{Node, request, start};

pub(crate) struct Held(pub(crate) HashMap<&'static str, &'static str>);

impl PipelineSource for Held {
    fn reserve_bytes(&self) -> usize {
        0
    }
    fn resolve(&self, id: &str) -> Result<SearchPipeline, PipelineFault> {
        let body = self.0.get(id).ok_or(PipelineFault::Missing)?;
        SearchPipeline::compile(body.as_bytes())
            .map_err(|e| PipelineFault::Broken(format!("{e:?}")))
    }
}

const PAIR: &str = r#"{"request_processors":[{"oversample":{"sample_factor":2}}],"response_processors":[{"truncate_hits":{}}]}"#;
const OVER: &str = r#"{"request_processors":[{"oversample":{"sample_factor":2}}]}"#;
pub(crate) const CUT: &str = r#"{"response_processors":[{"truncate_hits":{"target_size":1}}]}"#;
const BARE: &str = r#"{"response_processors":[{"truncate_hits":{}}]}"#;
const FILTER: &str =
    r#"{"request_processors":[{"filter_query":{"query":{"term":{"title":"sugar"}}}}]}"#;

fn node() -> Node {
    let node = start();
    let held = Held(HashMap::from([
        ("pair", PAIR),
        ("over", OVER),
        ("cut", CUT),
        ("bare", BARE),
        ("filter", FILTER),
    ]));
    assert!(node.service.set_search_pipelines(Arc::new(held)));
    node
}

fn search(node: &Node, index: &str, query: &str, body: &str) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        &format!("/{index}/_search{query}"),
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn returned(body: &str) -> usize {
    body.matches(r#""_index":"#).count()
}

fn total(body: &str) -> u64 {
    let (_, rest) = body.split_once(r#""total":{"value":"#).expect("a total");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a number")
}

// AC-6
#[test]
fn oversample_asks_for_more_and_truncate_cuts_back() {
    let node = node();
    let body = r#"{"size":1,"query":{"match_all":{}}}"#;
    let (s, plain) = search(&node, "compat-term", "", body);
    assert_eq!((s, returned(&plain)), (200, 1), "{plain}");
    let (s, over) = search(&node, "compat-term", "?search_pipeline=over", body);
    assert_eq!(
        (s, returned(&over)),
        (200, 2),
        "oversample alone asks for two: {over}"
    );
    let (s, pair) = search(&node, "compat-term", "?search_pipeline=pair", body);
    assert_eq!((s, returned(&pair)), (200, 1), "{pair}");
    assert_eq!(total(&pair), total(&plain));
}

// AC-7
#[test]
fn truncate_hits_cuts_to_its_target_and_fails_without_one() {
    let node = node();
    let (s, cut) = search(
        &node,
        "compat-term",
        "?search_pipeline=cut",
        r#"{"size":10}"#,
    );
    assert_eq!((s, returned(&cut), total(&cut)), (200, 1, 5), "{cut}");
    let (s, bare) = search(&node, "compat-term", "?search_pipeline=bare", "{}");
    assert_eq!(s, 500, "{bare}");
    assert!(
        bare.contains("search_pipeline_processing_exception") && bare.contains("target_size"),
        "{bare}"
    );
}

// AC-8
#[test]
fn filter_query_narrows_a_search() {
    let node = node();
    let (_, all) = search(&node, "compat-term", "", r#"{"query":{"match_all":{}}}"#);
    let (s, narrowed) = search(
        &node,
        "compat-term",
        "?search_pipeline=filter",
        r#"{"query":{"match_all":{}}}"#,
    );
    assert_eq!(s, 200, "{narrowed}");
    assert!(
        total(&narrowed) < total(&all) && total(&narrowed) >= 1,
        "{narrowed}"
    );
    let (s, none) = search(&node, "compat-term", "?search_pipeline=filter", "{}");
    assert_eq!((s, total(&none)), (200, total(&narrowed)), "{none}");
}

// AC-9
#[cfg(feature = "coordinator")]
#[test]
fn a_sharded_index_cuts_the_merged_hits() {
    use crate::support::{put_shard_copy, put_variant};
    let node = node();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let body = r#"{"size":1,"query":{"match_all":{}}}"#;
    let (s, plain) = search(&node, "two", "", body);
    assert_eq!((s, returned(&plain)), (200, 1), "{plain}");
    let (s, over) = search(&node, "two", "?search_pipeline=over", body);
    assert_eq!((s, returned(&over)), (200, 2), "{over}");
    let (s, pair) = search(&node, "two", "?search_pipeline=pair", body);
    assert_eq!((s, returned(&pair)), (200, 1), "{pair}");
    let (s, cut) = search(
        &node,
        "two",
        "?search_pipeline=cut",
        r#"{"size":10,"query":{"match_all":{}}}"#,
    );
    assert_eq!((s, returned(&cut)), (200, 1), "{cut}");
}

// AC-10
#[test]
fn a_missing_pipeline_is_400_and_none_runs_nothing() {
    let node = node();
    let (s, missing) = search(&node, "compat-term", "?search_pipeline=nope", "{}");
    assert_eq!(s, 400, "{missing}");
    assert!(
        missing.contains("pipeline_missing") && missing.contains("no such search pipeline: nope"),
        "{missing}"
    );
    let (_, plain) = search(&node, "compat-term", "", r#"{"size":1}"#);
    let (s, none) = search(
        &node,
        "compat-term",
        "?search_pipeline=_none",
        r#"{"size":1}"#,
    );
    assert_eq!(s, 200, "{none}");
    assert_eq!(returned(&none), returned(&plain));
}

// AC-5
#[test]
fn without_a_pipeline_source_naming_one_is_501() {
    let node = start();
    let (s, body) = search(&node, "compat-term", "?search_pipeline=p", "{}");
    assert_eq!(s, 501, "{body}");
    assert!(body.contains("not_implemented"), "{body}");
    assert_eq!(
        search(&node, "compat-term", "?search_pipeline=_none", "{}").0,
        200
    );
}

// AC-6: the cut keeps what belongs to each hit it keeps.
#[test]
fn a_cut_keeps_the_explanations_and_the_page_after_from() {
    let node = node();
    let body = r#"{"size":2,"explain":true,"query":{"match_all":{}}}"#;
    let (s, cut) = search(&node, "compat-term", "?search_pipeline=cut", body);
    assert_eq!((s, returned(&cut)), (200, 1), "{cut}");
    assert_eq!(cut.matches(r#""_explanation""#).count(), 1, "{cut}");
    assert_eq!(
        cut.matches(r#""_shard""#).count(),
        1,
        "an explained hit names its shard: {cut}"
    );
    let paged = r#"{"size":3,"from":1,"query":{"match_all":{}}}"#;
    let (_, plain) = search(&node, "compat-term", "", paged);
    let (_, cut) = search(&node, "compat-term", "?search_pipeline=cut", paged);
    let first = |b: &str| b.split_once(r#""_id":""#).map(|(_, r)| r[..1].to_owned());
    assert_eq!(
        first(&cut),
        first(&plain),
        "the cut page starts where the plain one does"
    );
    assert_eq!(returned(&cut), 1);
}
