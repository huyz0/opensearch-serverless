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

fn msearch(query: &str, header: &str) -> (u16, String) {
    let node = start();
    let body = format!("{header}\n{{}}\n");
    let r = request(
        node.addr(),
        "POST",
        &format!("/compat-paging-sort/_msearch{query}"),
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

#[test]
fn msearch_header_keys_that_tune_a_search_are_accepted_without_effect() {
    for header in [
        r#"{"search_type":"query_then_fetch"}"#,
        r#"{"request_cache":true}"#,
        r#"{"ccs_minimize_roundtrips":"true"}"#,
        r#"{"allow_no_indices":true}"#,
        r#"{"expand_wildcards":"open"}"#,
        r#"{"max_concurrent_shard_requests":5}"#,
        r#"{"pre_filter_shard_size":1}"#,
    ] {
        let (status, text) = msearch("", header);
        assert_eq!(status, 200, "{header}: {text}");
        assert!(text.contains(r#""status":200"#), "{header}: {text}");
        assert!(text.contains(r#""total":{"value":5"#), "{header}: {text}");
    }
    let (_, text) = msearch("", r#"{"search_type":"dfs_query_then_fetch"}"#);
    assert!(text.contains("not supported by this node yet"), "{text}");
    let (_, text) = msearch("", r#"{"search_type":"nonsense"}"#);
    assert!(text.contains("No search type for [nonsense]"), "{text}");
    let (_, text) = msearch("", r#"{"expand_wildcards":"closed"}"#);
    assert!(text.contains("not supported by this node yet"), "{text}");
}

#[test]
fn msearch_url_parameters_that_tune_are_accepted_and_the_total_can_be_a_number() {
    for query in [
        "?max_concurrent_searches=2",
        "?max_concurrent_shard_requests=5",
        "?pre_filter_shard_size=1",
        "?search_type=query_then_fetch",
        "?ccs_minimize_roundtrips=true",
    ] {
        let (status, text) = msearch(query, "{}");
        assert_eq!(status, 200, "{query}: {text}");
        assert!(text.contains(r#""status":200"#), "{query}: {text}");
        assert!(text.contains(r#""total":{"value":5"#), "{query}: {text}");
    }
    let (status, text) = msearch("?rest_total_hits_as_int=true", "{}");
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""hits":{"total":5,"#), "{text}");
    let (status, text) = msearch("?search_type=dfs_query_then_fetch", "{}");
    assert_eq!(status, 400, "{text}");
    assert_eq!(msearch("?typed_keys=true", "{}").0, 400);
}

const TOP_HITS: &str = r#"{"size":0,"aggs":{"top":{"top_hits":{"size":1}},"by":{"terms":{"field":"year","size":1},"aggs":{"best":{"top_hits":{"size":1}}}}}}"#;

#[test]
fn the_total_inside_a_top_hits_is_a_number_too() {
    let (status, plain) = post("", TOP_HITS);
    assert_eq!(status, 200, "{plain}");
    // The response's own total and one in each of the two top_hits.
    assert_eq!(plain.matches(r#""total":{"value":"#).count(), 3, "{plain}");
    let (status, number) = post("rest_total_hits_as_int=true", TOP_HITS);
    assert_eq!(status, 200, "{number}");
    assert!(!number.contains(r#""total":{"value""#), "{number}");
    assert!(
        number.contains(r#""hits":{"total":5,"max_score""#),
        "{number}"
    );
    assert_eq!(number.matches(r#""hits":{"total":"#).count(), 3, "{number}");
    let (_, off) = post("rest_total_hits_as_int=false", TOP_HITS);
    assert_eq!(off.matches(r#""total":{"value":"#).count(), 3, "{off}");
}

#[test]
fn the_total_inside_a_top_hits_over_several_indexes_is_a_number_too() {
    let node = start();
    let run = |query: &str| {
        let r = request(
            node.addr(),
            "POST",
            &format!("/compat-term,compat-paging-sort/_search?{query}"),
            Some(TOP_HITS.as_bytes()),
        );
        assert_eq!(r.status, 200, "{}", r.text());
        r.text()
    };
    assert_eq!(run("").matches(r#""total":{"value":"#).count(), 3);
    let number = run("rest_total_hits_as_int=true");
    assert!(!number.contains(r#""total":{"value""#), "{number}");
    assert_eq!(number.matches(r#""hits":{"total":"#).count(), 3, "{number}");
}

#[test]
fn wildcard_options_are_accepted_when_they_change_nothing() {
    let plain = ok("size=2");
    for query in [
        "allow_no_indices=true",
        "ignore_throttled=false",
        "expand_wildcards=open",
    ] {
        assert_eq!(ok(&format!("size=2&{query}")), plain, "{query}");
    }
    for query in ["expand_wildcards=closed", "expand_wildcards=all"] {
        let (status, text) = get(query);
        assert_eq!(status, 400, "{query}: {text}");
        assert!(text.contains("not supported by this node yet"), "{text}");
    }
    let node = start();
    let path = "/compat-paging-sort/_count?allow_no_indices=true&ignore_throttled=false&expand_wildcards=open";
    let r = request(node.addr(), "GET", path, None);
    assert_eq!(r.status, 200, "{path}: {}", r.text());
    let r = request(
        node.addr(),
        "GET",
        "/compat-paging-sort/_count?expand_wildcards=closed",
        None,
    );
    assert_eq!(r.status, 400, "{}", r.text());
}

const METRICS: &str = r#"{"size":0,"aggs":{"lo":{"min":{"field":"year"}},"hi":{"max":{"field":"year"}},"n":{"value_count":{"field":"year"}},"avg_y":{"avg":{"field":"year"}},"s":{"stats":{"field":"year"}},"u":{"cardinality":{"field":"year"}},"p":{"percentiles":{"field":"year","percents":[50]}},"r":{"percentile_ranks":{"field":"year","values":[2000]}},"sm":{"sum":{"field":"year"}},"top":{"top_hits":{"size":1}}}}"#;

#[test]
fn typed_keys_prefixes_a_metric_aggregation_with_its_type() {
    let (status, plain) = post("", METRICS);
    assert_eq!(status, 200, "{plain}");
    let (status, typed) = post("typed_keys=true", METRICS);
    assert_eq!(status, 200, "{typed}");
    for (name, prefix) in [
        ("lo", "min"),
        ("hi", "max"),
        ("n", "value_count"),
        ("avg_y", "avg"),
        ("s", "stats"),
        ("u", "cardinality"),
        ("p", "tdigest_percentiles"),
        ("r", "tdigest_percentile_ranks"),
        ("sm", "sum"),
        ("top", "top_hits"),
    ] {
        assert!(
            typed.contains(&format!(r#""{prefix}#{name}":{{"#)),
            "{prefix}#{name}: {typed}"
        );
        assert!(
            plain.contains(&format!(r#""{name}":{{"#)),
            "{name}: {plain}"
        );
    }
    // The values and their order are the plain answer's.
    let untyped = typed
        .replace("tdigest_percentile_ranks#", "")
        .replace("tdigest_percentiles#", "")
        .replace("value_count#", "")
        .replace("cardinality#", "")
        .replace("top_hits#", "")
        .replace("stats#", "")
        .replace("min#", "")
        .replace("max#", "")
        .replace("avg#", "")
        .replace("sum#", "");
    assert_eq!(
        untyped.split_once(r#""aggregations""#).map(|p| p.1),
        plain.split_once(r#""aggregations""#).map(|p| p.1)
    );
    let (_, off) = post("typed_keys=false", METRICS);
    assert_eq!(
        off.split_once(r#""aggregations""#).map(|p| p.1),
        plain.split_once(r#""aggregations""#).map(|p| p.1)
    );
}

#[test]
fn typed_keys_without_aggregations_changes_nothing_and_a_non_boolean_is_refused() {
    assert_eq!(ok("typed_keys=true"), ok(""));
    let (status, text) = get("typed_keys=maybe");
    assert_eq!(status, 400, "{text}");
}

#[test]
fn typed_keys_over_several_indexes_prefixes_the_merged_aggregations() {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-term,compat-paging-sort/_search?typed_keys=true",
        Some(br#"{"size":0,"aggs":{"hi":{"max":{"field":"year"}},"n":{"value_count":{"field":"year"}}}}"#),
    );
    let text = r.text();
    assert_eq!(r.status, 200, "{text}");
    assert!(text.contains(r#""max#hi":{"#), "{text}");
    assert!(text.contains(r#""value_count#n":{"#), "{text}");
}

const BUCKETS: &str = r#"{"size":0,"aggs":{
  "t":{"terms":{"field":"year"},"aggs":{"m":{"max":{"field":"year"}},"inner":{"terms":{"field":"tag"}}}},
  "k":{"terms":{"field":"tag"},"aggs":{"c":{"value_count":{"field":"year"}}}},
  "h":{"histogram":{"field":"year","interval":5},"aggs":{"s":{"sum":{"field":"year"}}}},
  "d":{"date_histogram":{"field":"published","calendar_interval":"year"}},
  "r":{"range":{"field":"year","ranges":[{"to":2000},{"from":2000}]}},
  "fs":{"filters":{"filters":{"a":{"term":{"tag":"animal"}}}},"aggs":{"lo":{"min":{"field":"year"}}}},
  "f":{"filter":{"term":{"tag":"animal"}},"aggs":{"hi":{"max":{"field":"year"}}}},
  "cp":{"composite":{"sources":[{"y":{"terms":{"field":"year"}}}]}}}}"#;

#[test]
fn typed_keys_prefixes_a_bucket_aggregation_and_its_sub_aggregations() {
    let (status, typed) = post("typed_keys=true", BUCKETS);
    assert_eq!(status, 200, "{typed}");
    for key in [
        "lterms#t",
        "max#m",
        "sterms#inner",
        "sterms#k",
        "value_count#c",
        "histogram#h",
        "sum#s",
        "date_histogram#d",
        "range#r",
        "filters#fs",
        "min#lo",
        "filter#f",
        "max#hi",
        "composite#cp",
    ] {
        assert!(typed.contains(&format!(r#""{key}":"#)), "{key}: {typed}");
    }
    let (_, plain) = post("", BUCKETS);
    for key in [
        "t", "m", "inner", "k", "c", "h", "s", "d", "r", "fs", "lo", "f", "hi", "cp",
    ] {
        assert!(plain.contains(&format!(r#""{key}":"#)), "{key}: {plain}");
    }
    assert!(!plain.contains('#'), "{plain}");
}

#[test]
fn typed_keys_over_several_indexes_prefixes_a_merged_bucket_aggregation() {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-term,compat-paging-sort/_search?typed_keys=true",
        Some(br#"{"size":0,"aggs":{"t":{"terms":{"field":"year"},"aggs":{"m":{"max":{"field":"year"}}}}}}"#),
    );
    let text = r.text();
    assert_eq!(r.status, 200, "{text}");
    assert!(
        text.contains(r#""lterms#t":{"#) && text.contains(r#""max#m":{"#),
        "{text}"
    );
}

#[test]
fn typed_keys_tells_missing_nested_and_reverse_nested_from_filter() {
    let body = r#"{"size":0,"aggs":{
      "ms":{"missing":{"field":"tag"}},
      "f":{"filter":{"term":{"tag":"animal"}}},
      "n":{"nested":{"path":"nowhere"},"aggs":{"back":{"reverse_nested":{}},"m":{"max":{"field":"nowhere.x"}}}}}}"#;
    let (status, typed) = post("typed_keys=true", body);
    assert_eq!(status, 200, "{typed}");
    for key in [
        "missing#ms",
        "filter#f",
        "nested#n",
        "reverse_nested#back",
        "max#m",
    ] {
        assert!(typed.contains(&format!(r#""{key}":"#)), "{key}: {typed}");
    }
    let (status, plain) = post("", body);
    assert_eq!(status, 200, "{plain}");
    assert!(!plain.contains('#'), "{plain}");
}
