//! docs/specs/search-url-params: the URL parameters of `_search` that set what
//! a body key sets.

use crate::support::{request, start};

const PATH: &str = "/compat-paging-sort/_search";

fn get(query: &str) -> (u16, String) {
    let node = start();
    let r = request(node.addr(), "GET", &format!("{PATH}?{query}"), None);
    (r.status, r.text())
}

fn post(query: &str, body: &str) -> (u16, String) {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        &format!("{PATH}?{query}"),
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

/// The answer without its `took`, which differs between two runs of one search.
fn untimed(text: &str) -> String {
    let from = text.find(r#""took":"#).expect("took") + 7;
    let end = from + text[from..].find(',').expect("end of took");
    format!("{}{}", &text[..from], &text[end..])
}

fn ok(query: &str) -> String {
    let (status, text) = get(query);
    assert_eq!(status, 200, "{query}: {text}");
    untimed(&text)
}

#[test]
fn track_total_hits_sets_the_total_and_beats_the_body() {
    assert!(!ok("track_total_hits=false").contains(r#""hits":{"total""#));
    assert!(ok("track_total_hits=1").contains(r#""total":{"value":1,"relation":"gte"}"#));
    assert!(ok("track_total_hits=true").contains(r#""relation":"eq""#));
    let (status, text) = post("track_total_hits=false", r#"{"track_total_hits":true}"#);
    assert_eq!(status, 200, "{text}");
    assert!(!text.contains(r#""hits":{"total""#), "{text}");
}

#[test]
fn explain_version_and_sequence_numbers_decorate_each_hit() {
    assert!(ok("explain=true").contains(r#""_explanation""#));
    assert!(!ok("explain=false").contains(r#""_explanation""#));
    assert!(ok("version=true").contains(r#""_version""#));
    let seq = ok("seq_no_primary_term=true");
    assert!(seq.contains(r#""_seq_no""#) && seq.contains(r#""_primary_term""#));
    let scored = ok("sort=year&track_scores=true");
    assert!(!scored.contains(r#""_score":null"#), "{scored}");
    assert!(ok("sort=year").contains(r#""_score":null"#));
}

#[test]
fn terminate_after_stops_collecting_and_refuses_a_negative() {
    // Only a scoring query can stop early so far.
    let body = r#"{"query":{"match":{"title":"quick"}}}"#;
    let (status, text) = post("terminate_after=1", body);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""terminated_early":true"#), "{text}");
    let (status, text) = post("terminate_after=-1", body);
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("terminateAfter must be > 0"), "{text}");
}

#[test]
fn source_parameters_filter_each_hit() {
    assert!(!ok("_source=false").contains(r#""_source""#));
    for query in ["_source=tag", "_source_includes=tag"] {
        let text = ok(query);
        assert!(
            text.contains(r#""tag""#) && !text.contains(r#""year""#),
            "{query}: {text}"
        );
    }
    let text = ok("_source_excludes=tag");
    assert!(
        text.contains(r#""year""#) && !text.contains(r#""tag""#),
        "{text}"
    );
}

#[test]
fn sort_orders_the_hits_and_replaces_the_body() {
    let desc = ok("sort=year:desc");
    let asc = ok("sort=year:asc");
    assert!(desc.contains(r#""sort":[2002"#), "{desc}");
    assert!(asc.contains(r#""sort":[1999"#), "{asc}");
    assert_eq!(ok("sort=year"), asc);
    let (status, text) = post("sort=year:desc", r#"{"sort":[{"year":"asc"}]}"#);
    assert_eq!(status, 200, "{text}");
    assert_eq!(untimed(&text), desc, "the URL sort replaces the body's");
    let (status, text) = get("sort=year:sideways");
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("sort direction [sideways] not supported"),
        "{text}"
    );
}

#[test]
fn tuning_parameters_change_nothing() {
    let plain = ok("size=3");
    for query in [
        "timeout=5s",
        "request_cache=true",
        "batched_reduce_size=512",
        "pre_filter_shard_size=1",
        "allow_partial_search_results=true",
        "ccs_minimize_roundtrips=true",
        "max_concurrent_shard_requests=5",
        "search_type=query_then_fetch",
    ] {
        assert_eq!(ok(&format!("size=3&{query}")), plain, "{query}");
    }
}

#[test]
fn what_is_not_served_is_still_refused() {
    let (status, text) = get("search_type=dfs_query_then_fetch");
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("not supported by this node yet"), "{text}");
    assert_eq!(get("typed_keys=true").0, 400);
}

#[test]
fn a_malformed_value_is_refused_as_opensearch_refuses_it() {
    let (status, text) = get("explain=maybe");
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Failed to parse value [maybe] as only [true] or [false] are allowed."),
        "{text}"
    );
    let (status, text) = get("track_total_hits=lots");
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("lots"), "{text}");
}

#[test]
fn rest_total_hits_as_int_writes_the_total_as_a_number() {
    let text = ok("rest_total_hits_as_int=true");
    assert!(text.contains(r#""hits":{"total":5,"max_score""#), "{text}");
    assert!(ok("rest_total_hits_as_int=false").contains(r#""total":{"value":5"#));
    // Counted exactly, whatever the default cap.
    let (status, text) = get("rest_total_hits_as_int=true&track_total_hits=true");
    assert_eq!(status, 200, "{text}");
    for query in [
        "rest_total_hits_as_int=true&track_total_hits=3",
        "rest_total_hits_as_int=true&track_total_hits=false",
    ] {
        let (status, text) = get(query);
        assert_eq!(status, 400, "{query}: {text}");
        assert!(
            text.contains("[rest_total_hits_as_int] cannot be used if the tracking of total hits is not accurate"),
            "{text}"
        );
    }
}

#[test]
fn the_total_of_a_search_over_several_indexes_is_a_number_too() {
    let node = start();
    let r = request(
        node.addr(),
        "GET",
        "/compat-term,compat-bool/_search?rest_total_hits_as_int=true",
        None,
    );
    let text = r.text();
    assert_eq!(r.status, 200, "{text}");
    assert!(!text.contains(r#""total":{"value""#), "{text}");
    assert!(text.contains(r#""hits":{"total":"#), "{text}");
    let r = request(
        node.addr(),
        "GET",
        "/nothing-here/_search?ignore_unavailable=true&rest_total_hits_as_int=true",
        None,
    );
    assert!(r.text().contains(r#""hits":{"total":0,"#), "{}", r.text());
}
