//! The search pipeline engine: what a definition may hold and what its
//! processors do to a search ([search-pipelines](../../../docs/specs/search-pipelines/SPEC.md)).

use oss_reader::dsl::{Query, Recorder, SearchSource};
use oss_reader::search_pipeline::{Refused, SearchPipeline};
use oss_rest::json::{Limits, Tokenizer};

fn source(doc: &str) -> SearchSource {
    let mut rec = Recorder::new(1 << 20);
    let mut t = Tokenizer::new(Limits::default());
    t.feed(doc.as_bytes(), &mut rec).expect("feeds");
    t.finish(&mut rec).expect("finishes");
    SearchSource::parse(&rec).expect("parses")
}

fn compile(def: &str) -> Result<SearchPipeline, Refused> {
    SearchPipeline::compile(def.as_bytes())
}

fn invalid(def: &str) -> String {
    match compile(def) {
        Err(Refused::Invalid(m)) => m,
        other => panic!("expected an invalid definition, got {:?}", other.map(drop)),
    }
}

fn unsupported(def: &str) -> String {
    match compile(def) {
        Err(Refused::Unsupported(m)) => m,
        other => panic!("expected an unsupported one, got {:?}", other.map(drop)),
    }
}

const BOTH: &str = r#"{"request_processors":[{"oversample":{"sample_factor":2.0}}],
 "response_processors":[{"truncate_hits":{}}]}"#;

#[test]
fn a_definition_of_supported_processors_is_accepted() {
    compile(BOTH).expect("both");
    compile(r#"{"description":"d","version":3,"request_processors":[{"filter_query":{"tag":"t","description":"x","query":{"term":{"a":1}}}}]}"#)
        .expect("filter");
    compile(r#"{"response_processors":[{"truncate_hits":{"target_size":2,"context_prefix":"p","ignore_failure":false}}]}"#)
        .expect("truncate");
}

#[test]
fn a_bad_definition_is_invalid() {
    assert!(invalid("[]").contains("object"));
    assert!(invalid(r#"{"request_processors":{}}"#).contains("array"));
    assert!(invalid(r#"{"request_processors":[{"nope":{}}]}"#).contains("[nope]"));
    assert!(
        invalid(r#"{"request_processors":[{"oversample":{},"filter_query":{}}]}"#)
            .contains("one type")
    );
    // A response processor in the request list is no processor of that kind.
    assert!(
        invalid(r#"{"request_processors":[{"truncate_hits":{}}]}"#).contains("[truncate_hits]")
    );
    assert!(
        invalid(r#"{"request_processors":[{"oversample":{"sample_factor":0.5}}]}"#)
            .contains("sample_factor")
    );
    assert!(invalid(r#"{"request_processors":[{"oversample":{}}]}"#).contains("sample_factor"));
    assert!(
        invalid(r#"{"request_processors":[{"oversample":{"sample_factor":2,"bogus":1}}]}"#)
            .contains("[bogus]")
    );
    assert!(
        invalid(r#"{"response_processors":[{"truncate_hits":{"target_size":-1}}]}"#)
            .contains("target_size")
    );
    assert!(invalid(r#"{"request_processors":[{"filter_query":{}}]}"#).contains("query"));
    assert!(
        invalid(r#"{"request_processors":[{"filter_query":{"query":{"nonesuch":{}}}}]}"#)
            .contains("nonesuch")
    );
    assert!(invalid(r#"{"unknown_key":1}"#).contains("[unknown_key]"));
}

#[test]
fn what_opensearch_ships_and_this_node_does_not_build_is_unsupported_by_name() {
    for (def, name) in [
        (r#"{"request_processors":[{"script":{}}]}"#, "script"),
        (
            r#"{"request_processors":[{"acl_routing_search":{}}]}"#,
            "acl_routing_search",
        ),
        (
            r#"{"response_processors":[{"rename_field":{}}]}"#,
            "rename_field",
        ),
        (r#"{"response_processors":[{"collapse":{}}]}"#, "collapse"),
        (r#"{"response_processors":[{"sort":{}}]}"#, "sort"),
        (r#"{"response_processors":[{"split":{}}]}"#, "split"),
        (
            r#"{"phase_results_processors":[{"normalization-processor":{}}]}"#,
            "phase_results_processors",
        ),
        (
            r#"{"request_processors":[{"oversample":{"sample_factor":2,"ignore_failure":true}}]}"#,
            "ignore_failure",
        ),
    ] {
        let m = unsupported(def);
        assert!(m.contains(name), "{m}");
        assert!(
            m.contains("oversample") && m.contains("truncate_hits") && m.contains("filter_query"),
            "{m}"
        );
    }
    compile(r#"{"phase_results_processors":[]}"#).expect("an empty list is nothing");
}

#[test]
fn oversample_then_truncate_asks_for_more_and_cuts_back() {
    let pipe = compile(BOTH).expect("compiles");
    let mut s = source(r#"{"size":3}"#);
    pipe.apply(&mut s).expect("runs");
    assert_eq!(s.size, 6);
    assert_eq!(s.response_size_cap, Some(3));
    let mut s = source("{}");
    pipe.apply(&mut s).expect("runs");
    assert_eq!((s.size, s.response_size_cap), (20, Some(10)));
}

#[test]
fn the_factor_rounds_up() {
    let pipe =
        compile(r#"{"request_processors":[{"oversample":{"sample_factor":1.5}}]}"#).expect("c");
    let mut s = source(r#"{"size":3}"#);
    pipe.apply(&mut s).expect("runs");
    assert_eq!(s.size, 5);
    assert_eq!(s.response_size_cap, None);
}

#[test]
fn truncate_hits_uses_its_own_target_and_the_smallest_wins() {
    let pipe = compile(r#"{"response_processors":[{"truncate_hits":{"target_size":2}},{"truncate_hits":{"target_size":4}}]}"#).expect("c");
    let mut s = source(r#"{"size":10}"#);
    pipe.apply(&mut s).expect("runs");
    assert_eq!((s.size, s.response_size_cap), (10, Some(2)));
}

#[test]
fn truncate_without_a_target_or_an_earlier_oversample_fails() {
    let pipe = compile(r#"{"response_processors":[{"truncate_hits":{}}]}"#).expect("c");
    let err = pipe.apply(&mut source("{}")).expect_err("fails");
    assert!(err.to_string().contains("target_size"), "{err}");
}

#[test]
fn a_context_prefix_keeps_pairs_apart() {
    let pipe = compile(
        r#"{"request_processors":[{"oversample":{"sample_factor":2,"context_prefix":"a"}}],
            "response_processors":[{"truncate_hits":{"context_prefix":"b"}}]}"#,
    )
    .expect("c");
    assert!(pipe.apply(&mut source("{}")).is_err());
    let pipe = compile(
        r#"{"request_processors":[{"oversample":{"sample_factor":2,"context_prefix":"a"}}],
            "response_processors":[{"truncate_hits":{"context_prefix":"a"}}]}"#,
    )
    .expect("c");
    let mut s = source("{}");
    pipe.apply(&mut s).expect("runs");
    assert_eq!(s.response_size_cap, Some(10));
}

#[test]
fn filter_query_narrows_a_query_and_a_search_with_none() {
    let pipe =
        compile(r#"{"request_processors":[{"filter_query":{"query":{"term":{"tag":"x"}}}}]}"#)
            .expect("c");
    let mut with = source(r#"{"query":{"match_all":{}}}"#);
    pipe.apply(&mut with).expect("runs");
    let Some(Query::Bool(b)) = &with.query else {
        panic!("{:?}", with.query)
    };
    assert_eq!((b.must.len(), b.filter.len()), (1, 1));
    assert!(matches!(b.must[0], Query::MatchAll { .. }));
    assert!(matches!(b.filter[0], Query::Term { .. }));
    let mut without = source("{}");
    pipe.apply(&mut without).expect("runs");
    let Some(Query::Bool(b)) = &without.query else {
        panic!("{:?}", without.query)
    };
    assert_eq!((b.must.len(), b.filter.len()), (0, 1));
    assert!(b.adjust_pure_negative);
}
