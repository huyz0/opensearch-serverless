//! docs/specs/rank-eval-api: `_rank_eval` end to end.

use std::sync::Arc;

use oss_node::search::{TemplateFault, TemplateSource};

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

const BROWN: &str = r#"{"query":{"term":{"title":"brown"}}}"#;
/// What the `brown` search ranks: document 4, then 1.
const HITS: &str = r#"[{"hit":{"_index":"compat-term","_id":"4","_score":0.52695787},"rating":1},{"hit":{"_index":"compat-term","_id":"1","_score":0.41331142},"rating":null}]"#;

fn eval(node: &Node, index: &str, body: &str) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        &format!("/{index}/_rank_eval"),
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn spec(request: &str, metric: &str) -> String {
    format!(r#"{{"requests":[{request}],"metric":{metric}}}"#)
}

fn rated(id: &str, ratings: &str) -> String {
    format!(r#"{{"id":"{id}","request":{BROWN},"ratings":[{ratings}]}}"#)
}

const DOC4: &str = r#"{"_index":"compat-term","_id":"4","rating":1}"#;

// AC-1
#[test]
fn precision_scores_the_relevant_share_of_the_hits() {
    let node = start();
    let body = spec(&rated("q", DOC4), r#"{"precision":{"k":5}}"#);
    assert_eq!(
        eval(&node, "compat-term", &body),
        (
            200,
            format!(
                r#"{{"metric_score":0.5,"details":{{"q":{{"metric_score":0.5,"unrated_docs":[{{"_index":"compat-term","_id":"1"}}],"hits":{HITS},"metric_details":{{"precision":{{"relevant_docs_retrieved":1,"docs_retrieved":2}}}}}}}},"failures":{{}}}}"#
            )
        )
    );
    // Leaving unlabeled hits out leaves the one rated hit, which is relevant.
    let body = spec(
        &rated("q", DOC4),
        r#"{"precision":{"k":5,"ignore_unlabeled":true}}"#,
    );
    let (status, text) = eval(&node, "compat-term", &body);
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with(r#"{"metric_score":1.0,"#), "{text}");
}

// AC-1
#[test]
fn recall_and_reciprocal_rank_score_against_the_ratings() {
    let node = start();
    let ratings = format!(r#"{DOC4},{{"_index":"compat-term","_id":"9","rating":2}}"#);
    let (status, text) = eval(
        &node,
        "compat-term",
        &spec(&rated("q", &ratings), r#"{"recall":{"k":5}}"#),
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with(r#"{"metric_score":0.5,"#), "{text}");
    assert!(
        text.contains(
            r#""metric_details":{"recall":{"relevant_docs_retrieved":1,"relevant_docs":2}}"#
        ),
        "{text}"
    );
    let rated_1 = r#"{"_index":"compat-term","_id":"1","rating":3}"#;
    let (status, text) = eval(
        &node,
        "compat-term",
        &spec(
            &rated("q", rated_1),
            r#"{"mean_reciprocal_rank":{"relevant_rating_threshold":2}}"#,
        ),
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with(r#"{"metric_score":0.5,"#), "{text}");
    assert!(
        text.contains(r#""metric_details":{"mean_reciprocal_rank":{"first_relevant":2}}"#),
        "{text}"
    );
}

// AC-10
#[test]
fn dcg_discounts_by_rank_and_ndcg_divides_by_the_best_order() {
    let node = start();
    // Hits 4 then 1: 1 / log2(2) + 7 / log2(3).
    let ratings = r#"{"_index":"compat-term","_id":"1","rating":3},{"_index":"compat-term","_id":"4","rating":1}"#;
    let (status, text) = eval(
        &node,
        "compat-term",
        &spec(&rated("q", ratings), r#"{"dcg":{"k":5}}"#),
    );
    assert_eq!(status, 200, "{text}");
    assert!(
        text.starts_with(r#"{"metric_score":5.416508275000202,"#),
        "{text}"
    );
    assert!(
        text.contains(r#""metric_details":{"dcg":{"dcg":5.416508275000202,"unrated_docs":0}}"#),
        "{text}"
    );
    let (status, text) = eval(
        &node,
        "compat-term",
        &spec(&rated("q", ratings), r#"{"dcg":{"normalize":true}}"#),
    );
    assert_eq!(status, 200, "{text}");
    assert!(
        text.starts_with(r#"{"metric_score":0.7098097413968654,"#),
        "{text}"
    );
    assert!(
        text.contains(
            r#""metric_details":{"dcg":{"dcg":5.416508275000202,"ideal_dcg":7.630929753571458,"normalized_dcg":0.7098097413968654,"unrated_docs":0}}"#
        ),
        "{text}"
    );
    // A hit with no rating counts as the unknown rating, and is listed unrated.
    let (status, text) = eval(
        &node,
        "compat-term",
        &spec(
            &rated("q", DOC4),
            r#"{"dcg":{"unknown_doc_rating":0,"k":5}}"#,
        ),
    );
    assert_eq!(status, 200, "{text}");
    assert!(
        text.contains(r#""metric_details":{"dcg":{"dcg":1.0,"unrated_docs":1}}"#),
        "{text}"
    );
}

// AC-1
#[test]
fn the_score_is_the_mean_of_the_requests() {
    let node = start();
    let none = r#"{"id":"none","request":{"query":{"term":{"title":"zzz"}}},"ratings":[]}"#;
    let body = format!(
        r#"{{"requests":[{},{none}],"metric":{{"mean_reciprocal_rank":{{}}}}}}"#,
        rated("q", DOC4)
    );
    let (status, text) = eval(&node, "compat-term", &body);
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with(r#"{"metric_score":0.5,"#), "{text}");
    assert!(
        text.contains(r#""none":{"metric_score":0.0,"unrated_docs":[],"hits":[]"#),
        "{text}"
    );
    assert!(text.contains(r#""first_relevant":-1"#), "{text}");
}

// AC-2
#[test]
fn a_template_request_renders_then_ranks() {
    let node = start();
    let body = r#"{"templates":[{"id":"t","template":{"source":"{\"query\":{\"term\":{\"title\":\"{{w}}\"}}}"}}],"requests":[{"id":"q","template_id":"t","params":{"w":"brown"},"ratings":[{"_index":"compat-term","_id":"4","rating":1}]}],"metric":{"precision":{}}}"#;
    let (status, text) = eval(&node, "compat-term", body);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(HITS), "{text}");
}

/// One stored template, `term`, and none else.
struct Stored;

impl TemplateSource for Stored {
    fn reserve_bytes(&self) -> usize {
        0
    }
    fn resolve(&self, id: &str) -> Result<String, TemplateFault> {
        match id {
            "term" => Ok(r#"{"query":{"term":{"title":"{{w}}"}}}"#.to_owned()),
            _ => Err(TemplateFault::Missing),
        }
    }
}

// AC-9
#[test]
fn a_template_naming_a_stored_script_ranks_like_its_inline_twin() {
    let node = start();
    assert!(node.service.set_stored_templates(Arc::new(Stored)));
    let body = |script: &str| {
        format!(
            r#"{{"templates":[{{"id":"t","template":{{"id":"{script}"}}}}],"requests":[{{"id":"q","template_id":"t","params":{{"w":"brown"}},"ratings":[{{"_index":"compat-term","_id":"4","rating":1}}]}}],"metric":{{"precision":{{}}}}}}"#
        )
    };
    let (status, text) = eval(&node, "compat-term", &body("term"));
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(HITS), "{text}");
    // A script that does not exist is the template read's own 404.
    let (status, text) = eval(&node, "compat-term", &body("nope"));
    assert_eq!(status, 404, "{text}");
    assert!(text.contains("unable to find script [nope]"), "{text}");
}

// AC-9
#[test]
fn a_stored_template_without_a_source_set_is_refused() {
    let node = start();
    let body = r#"{"templates":[{"id":"t","template":{"id":"term"}}],"requests":[{"id":"q","template_id":"t","params":{"w":"brown"},"ratings":[]}],"metric":{"precision":{}}}"#;
    let (status, text) = eval(&node, "compat-term", body);
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("not supported by this node yet"), "{text}");
}

// AC-3
#[cfg(feature = "coordinator")]
#[test]
fn a_two_shard_index_ranks_through_the_coordinator() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let body = spec(
        r#"{"id":"q","request":{"query":{"match_all":{}}},"ratings":[]}"#,
        r#"{"precision":{"k":20}}"#,
    );
    let (status, text) = eval(&node, "two", &body);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""failures":{}"#), "{text}");
}

// AC-4
#[test]
fn a_search_that_fails_is_reported_under_failures() {
    let node = start();
    let body = spec(&rated("q", DOC4), r#"{"precision":{}}"#);
    let (status, text) = eval(&node, "no-such-index", &body);
    assert_eq!(status, 200, "{text}");
    assert!(
        text.starts_with(r#"{"metric_score":"NaN","details":{},"failures":{"q":{"error":{"#),
        "{text}"
    );
    assert!(text.contains("index_not_found_exception"), "{text}");
    assert!(!text.contains(r#""status""#), "{text}");
}

// AC-5
#[test]
fn a_request_that_is_not_a_valid_evaluation_is_a_400() {
    let node = start();
    let ok = rated("q", DOC4);
    let bad = [
        // no requests
        r#"{"requests":[],"metric":{"precision":{}}}"#.to_owned(),
        // no metric
        format!(r#"{{"requests":[{ok}]}}"#),
        // unknown key
        format!(r#"{{"requests":[{ok}],"metric":{{"precision":{{}}}},"x":1}}"#),
        // k and threshold out of range
        spec(&ok, r#"{"precision":{"k":0}}"#),
        spec(&ok, r#"{"recall":{"relevant_rating_threshold":-1}}"#),
        // neither a request nor a template
        spec(r#"{"id":"q","ratings":[]}"#, r#"{"precision":{}}"#),
        // both a request and params
        spec(
            &format!(
                r#"{{"id":"q","request":{BROWN},"params":{{"a":1}},"template_id":"t","ratings":[]}}"#
            ),
            r#"{"precision":{}}"#,
        ),
        // the same document rated twice
        spec(
            &rated("q", &format!("{DOC4},{DOC4}")),
            r#"{"precision":{}}"#,
        ),
        // what a rated request may not ask for
        spec(
            r#"{"id":"q","request":{"explain":true},"ratings":[]}"#,
            r#"{"precision":{}}"#,
        ),
        // not JSON
        "{".to_owned(),
        String::new(),
    ];
    for body in bad {
        let (status, text) = eval(&node, "compat-term", &body);
        assert_eq!(status, 400, "{body}: {text}");
    }
    let (_, text) = eval(
        &node,
        "compat-term",
        &spec(
            r#"{"id":"q","request":{"explain":true},"ratings":[]}"#,
            r#"{"precision":{}}"#,
        ),
    );
    assert!(
        text.contains("Query in rated requests should not use explain."),
        "{text}"
    );
}

// AC-6
#[test]
fn what_this_node_lacks_is_refused() {
    let node = start();
    let ok = rated("q", DOC4);
    for (body, what) in [
        (
            spec(
                &ok,
                r#"{"expected_reciprocal_rank":{"maximum_relevance":3}}"#,
            ),
            "ERR",
        ),
        (
            spec(
                &format!(
                    r#"{{"id":"q","request":{BROWN},"ratings":[],"summary_fields":["title"]}}"#
                ),
                r#"{"precision":{}}"#,
            ),
            "summary_fields",
        ),
        (
            spec(
                r#"{"id":"q","request":{"sort":["_id"]},"ratings":[]}"#,
                r#"{"precision":{}}"#,
            ),
            "sort",
        ),
    ] {
        let (status, text) = eval(&node, "compat-term", &body);
        assert_eq!(status, 400, "{what}: {text}");
        assert!(
            text.contains("not supported by this node yet"),
            "{what}: {text}"
        );
    }
    let body = spec(&ok, r#"{"precision":{}}"#);
    for index in ["_all", "a,b", "comp*"] {
        assert_eq!(eval(&node, index, &body).0, 400, "{index}");
    }
    let r = request(node.addr(), "POST", "/_rank_eval", Some(body.as_bytes()));
    assert_eq!(r.status, 400, "{}", r.text());
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_rank_eval?search_type=dfs_query_then_fetch",
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 400, "{}", r.text());
}

// AC-7
#[test]
fn details_come_in_the_order_java_writes_them() {
    let node = start();
    let ids = ["a", "b", "c", "query-3", "zz"];
    let requests: Vec<String> = ids.iter().map(|id| rated(id, DOC4)).collect();
    let body = format!(
        r#"{{"requests":[{}],"metric":{{"precision":{{}}}}}}"#,
        requests.join(",")
    );
    let (status, text) = eval(&node, "compat-term", &body);
    assert_eq!(status, 200, "{text}");
    let at = |id: &str| text.find(&format!(r#""{id}":{{"metric_score""#)).expect(id);
    let mut by_position: Vec<&str> = ids.to_vec();
    by_position.sort_by_key(|id| at(id));
    // The order `new HashMap<>(5)` iterates these keys in, run on the JDK.
    assert_eq!(by_position, ["zz", "a", "b", "query-3", "c"]);
}

// AC-8
#[test]
fn a_warm_index_adds_no_store_request() {
    let node = start();
    let body = spec(&rated("q", DOC4), r#"{"precision":{}}"#);
    assert_eq!(eval(&node, "compat-term", &body).0, 200);
    let warm = counts(&node.store);
    assert_eq!(eval(&node, "compat-term", &body).0, 200);
    assert_eq!(counts(&node.store), warm);
}
