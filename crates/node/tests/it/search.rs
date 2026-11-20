//! `_search` answered end to end, and what it asks of the object store.

use std::thread;
use std::time::Duration;

use oss_blob::S3Op;

use crate::support::{counts, request, start};

fn since(before: [u64; 7], now: [u64; 7]) -> Vec<(S3Op, u64)> {
    S3Op::ALL
        .iter()
        .zip(now.iter().zip(before.iter()))
        .filter(|(_, (n, b))| n > b)
        .map(|(op, (n, b))| (*op, n - b))
        .collect()
}

#[test]
fn a_search_streams_hits_with_their_source() {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"query":{"term":{"title":"brown"}}}"#),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.header("content-type"),
        Some("application/json; charset=UTF-8")
    );
    assert_eq!(r.header("transfer-encoding"), Some("chunked"));
    let body = r.text();
    assert!(
        body.contains(r#""hits":{"total":{"value":2,"relation":"eq"},"max_score":0.52695787,"hits":[{"_index":"compat-term","_id":"4","_score":0.52695787,"_source":{"title":"Brown sugar and brown bread""#),
        "{body}"
    );
}

#[test]
fn an_index_opens_once_with_two_gets_and_no_list() {
    let node = start();
    let before = counts(&node.store);
    let r = request(node.addr(), "GET", "/compat-match-all/_search", None);
    assert_eq!(r.status, 200, "{}", r.text());
    let first = since(before, counts(&node.store));
    let get = |made: &[(S3Op, u64)], op| made.iter().find(|(o, _)| *o == op).map_or(0, |(_, n)| *n);
    assert_eq!(
        get(&first, S3Op::Get),
        2,
        "the descriptor and the commit: {first:?}"
    );
    assert_eq!(
        get(&first, S3Op::GetRange),
        1,
        "the files open and match_all touch, in one merged ranged read by the open prefetch: {first:?}"
    );
    for op in [S3Op::List, S3Op::Head, S3Op::Put, S3Op::Copy, S3Op::Delete] {
        assert_eq!(get(&first, op), 0, "{op:?}: {first:?}");
    }
    assert_eq!(node.service.open_indexes(), 1);
    let warm = counts(&node.store);
    let r = request(node.addr(), "GET", "/compat-match-all/_search", None);
    assert_eq!(r.status, 200);
    assert_eq!(
        since(warm, counts(&node.store)),
        [],
        "a second search is served from the open shard and the page cache"
    );
}

#[test]
fn an_idle_index_makes_no_requests() {
    let node = start();
    let r = request(node.addr(), "GET", "/compat-paging-sort/_search", None);
    assert_eq!(r.status, 200);
    let idle = counts(&node.store);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        since(idle, counts(&node.store)),
        [],
        "nothing polls an open index"
    );
}

#[test]
fn a_missing_index_is_not_found_after_one_get() {
    let node = start();
    let before = counts(&node.store);
    let r = request(node.addr(), "POST", "/compat-nope/_search", Some(b"{}"));
    assert_eq!(r.status, 404);
    assert!(
        r.text().contains(
            r#""type":"index_not_found_exception","reason":"no such index [compat-nope]""#
        ),
        "{}",
        r.text()
    );
    assert_eq!(since(before, counts(&node.store)), [(S3Op::Get, 1)]);
    assert_eq!(
        node.service.open_indexes(),
        0,
        "a missing index is not kept"
    );
}

#[test]
fn a_bad_body_is_refused_before_the_index_is_opened() {
    let node = start();
    let before = counts(&node.store);
    for (body, want) in [
        (&br#"{"query":{"nope":{}}}"#[..], "unknown query [nope]"),
        (b"{\"size\":", "json_parse_exception"),
        (b"[]", "Expected [START_OBJECT] but found [START_ARRAY]"),
    ] {
        let r = request(node.addr(), "POST", "/compat-term/_search", Some(body));
        assert_eq!(r.status, 400, "{}", r.text());
        assert!(r.text().contains(want), "{}", r.text());
    }
    let r = request(node.addr(), "GET", "/compat-term/_search?size=x", None);
    assert_eq!(r.status, 400);
    assert!(
        r.text()
            .contains("Failed to parse int parameter [size] with value [x]"),
        "{}",
        r.text()
    );
    assert_eq!(
        since(before, counts(&node.store)),
        [],
        "no request for a refused search"
    );
}

#[test]
fn shard_failures_name_the_node_and_the_index() {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"query":{"term":{"year":"abc"}}}"#),
    );
    assert_eq!(r.status, 400, "{}", r.text());
    let body = r.text();
    assert!(body.contains(r#""failed_shards":[{"shard":0,"index":"compat-term","node":"node-1","reason":{"type":"query_shard_exception","reason":"failed to create query: For input string: \"abc\"","index":"compat-term","index_uuid":""#), "{body}");
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"sort":[{"nope":"asc"}]}"#),
    );
    assert_eq!(r.status, 400);
    assert!(
        r.text()
            .contains("No mapping found for [nope] in order to sort on"),
        "{}",
        r.text()
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"sort":[{"title":"asc"}]}"#),
    );
    assert_eq!(r.status, 400);
    assert!(
        r.text().contains("Text fields are not optimised"),
        "{}",
        r.text()
    );
}

#[test]
fn sorts_put_missing_values_last_and_page() {
    let node = start();
    let sorted = |sort: &str| {
        let body = format!(r#"{{"size":2,"from":1,"sort":{sort}}}"#);
        let r = request(
            node.addr(),
            "POST",
            "/compat-paging-sort/_search",
            Some(body.as_bytes()),
        );
        assert_eq!(r.status, 200, "{}", r.text());
        r.text()
    };
    let by_year = sorted(r#"[{"year":"desc"},{"_doc":"asc"}]"#);
    assert!(by_year.contains(r#""_id":"2","_score":null"#), "{by_year}");
    assert!(by_year.contains(r#""sort":[2002,1]"#), "{by_year}");
    assert!(by_year.contains(r#""sort":[2002,3]"#), "{by_year}");
    let by_tag = sorted(r#"[{"tag":"desc"},{"_score":"asc"}]"#);
    assert!(by_tag.contains(r#""sort":["food",1.0]"#), "{by_tag}");
    let r = request(
        node.addr(),
        "POST",
        "/compat-paging-sort/_search",
        Some(br#"{"size":0,"sort":"year"}"#),
    );
    assert!(
        r.text().contains(r#""max_score":null,"hits":[]"#),
        "{}",
        r.text()
    );
}

#[test]
fn an_index_the_node_cannot_serve_is_refused() {
    let node = start();
    crate::support::put_variant(&node.store, "compat-term", "two-shards", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    crate::support::put_variant(&node.store, "compat-term", "corrupt", |_| "{".into());
    let r = request(node.addr(), "GET", "/two-shards/_search", None);
    // Without the coordinator an index of two shards is refused; with it
    // the second shard, whose blobs are not here, fails alone.
    #[cfg(not(feature = "coordinator"))]
    {
        assert_eq!(r.status, 400);
        assert!(
            r.text()
                .contains("searching an index of [2] shards is not supported by this node yet"),
            "{}",
            r.text()
        );
    }
    #[cfg(feature = "coordinator")]
    {
        assert_eq!(r.status, 200, "{}", r.text());
        assert!(
            r.text()
                .contains(r#""successful":1,"skipped":0,"failed":1"#)
        );
    }
    let r = request(node.addr(), "GET", "/corrupt/_search", None);
    assert_eq!(r.status, 500, "{}", r.text());
    assert!(r.text().contains(r#""type":"exception""#), "{}", r.text());
    assert_eq!(
        node.service.open_indexes(),
        usize::from(cfg!(feature = "coordinator"))
    );
}

#[test]
fn fields_the_reader_does_not_search_or_sort_yet_are_refused() {
    let node = start();
    crate::support::put_variant(&node.store, "compat-term", "extra-fields", |j| {
        j.replace(
            r#"\"published\":{\"type\":\"date\"}"#,
            r#"\"published\":{\"type\":\"date\"},\"geo\":{\"type\":\"geo_point\"},\"nd\":{\"type\":\"keyword\",\"doc_values\":false},\"n\":{\"type\":\"long\"}"#,
        )
    });
    let search = |body: &str| {
        let r = request(
            node.addr(),
            "POST",
            "/extra-fields/_search",
            Some(body.as_bytes()),
        );
        (r.status, r.text())
    };
    let (status, text) = search(r#"{"sort":"geo"}"#);
    assert_eq!(status, 400);
    assert!(
        text.contains("sorting on a [geo_point] field [geo] is not supported by this node yet"),
        "{text}"
    );
    let (status, text) = search(r#"{"sort":"nd"}"#);
    assert_eq!(status, 400);
    assert!(
        text.contains(
            "sorting on the field [nd] without doc values is not supported by this node yet"
        ),
        "{text}"
    );
    let (status, text) = search(r#"{"sort":[{"n":"desc"}],"size":1}"#);
    assert_eq!(status, 200, "{text}");
    assert!(
        text.contains(r#""sort":[-9223372036854775808]"#),
        "a missing long sorts last: {text}"
    );
    let (status, text) = search(r#"{"query":{"term":{"geo":"x"}}}"#);
    assert_eq!(status, 400);
    assert!(
        text.contains("searching a [geo_point] field [geo] is not supported by this node yet"),
        "{text}"
    );
}

fn small_pool(bytes: usize) -> oss_mem::MemoryBudget {
    oss_mem::MemoryBudget::new(
        &oss_mem::BudgetConfig::new(64 << 20)
            .with_pool(oss_mem::Pool::RequestBodies, bytes)
            .and_then(|c| c.with_pool(oss_mem::Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    )
}

#[test]
fn a_search_the_request_body_pool_cannot_hold_is_refused_with_429() {
    // The server's own buffers take 480 KiB of it; what is left is under
    // the 1 MiB a recording may grow to and under five hits' keyword sort
    // values, each reserved at the longest term.
    let node = crate::support::start_with(crate::support::store(), &small_pool(512 << 10));
    let mut body = br#"{"query":{"term":{"tag":""#.to_vec();
    body.resize(2 << 20, b'a');
    body.extend_from_slice(br#""}}}"#);
    let r = request(node.addr(), "POST", "/compat-term/_search", Some(&body));
    assert_eq!(r.status, 429, "{}", r.text());
    assert!(
        r.text().contains("circuit_breaking_exception"),
        "{}",
        r.text()
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"size":10000,"sort":"tag"}"#),
    );
    assert_eq!(
        r.status,
        429,
        "the window's hits are reserved first: {}",
        r.text()
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"size":10000,"sort":"year"}"#),
    );
    assert_eq!(
        r.status,
        200,
        "a window is no larger than the index: {}",
        r.text()
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"size":1}"#),
    );
    assert_eq!(r.status, 200, "a small search still fits: {}", r.text());
}

#[test]
fn a_node_with_no_open_index_pool_does_not_start() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = oss_mem::MemoryBudget::new(
        &oss_mem::BudgetConfig::new(64 << 20)
            .with_pool(oss_mem::Pool::RequestBodies, 16 << 20)
            .expect("fits"),
    );
    let started = oss_node::search::start_reader(
        listener,
        crate::support::server_config(),
        crate::support::store(),
        "",
        crate::support::NODE,
        &budget,
    );
    let Err(e) = started else {
        panic!("started");
    };
    assert_eq!(e.to_string(), "the open_indexes pool is empty");
}

#[test]
fn a_node_whose_pool_cannot_hold_the_server_buffers_does_not_start() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
    let started = oss_node::search::start_reader(
        listener,
        crate::support::server_config(),
        crate::support::store(),
        "",
        crate::support::NODE,
        &small_pool(1024),
    );
    let Err(e) = started else {
        panic!("started");
    };
    assert!(e.to_string().starts_with("server: Memory"), "{e}");
}
