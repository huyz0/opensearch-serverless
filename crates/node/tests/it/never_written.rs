//! An index that was created and never written answers its reads as an
//! index of no documents, as stock OpenSearch does, with no request past
//! the descriptor's and the commit register's reads.

use oss_blob::S3Op;

use crate::support::{Node, counts, put_variant, request, start};

const EMPTY: &str = "compat-never-written";

fn node_with_empty_index() -> Node {
    let node = start();
    put_variant(&node.store, "compat-term", EMPTY, |j| {
        j.replace("compat-term-uuid", "never-written-uuid")
            .replace(r#""name":"compat-term""#, &format!(r#""name":"{EMPTY}""#))
    });
    node
}

fn get(node: &Node, target: &str, body: Option<&str>) -> (u16, String) {
    let method = if body.is_some() { "POST" } else { "GET" };
    let r = request(node.addr(), method, target, body.map(str::as_bytes));
    (r.status, r.text())
}

const NO_HITS: &str = r#""hits":{"total":{"value":0,"relation":"eq"},"max_score":null,"hits":[]}"#;

#[test]
fn a_search_answers_200_with_no_hits() {
    let node = node_with_empty_index();
    for body in [
        None,
        Some(r#"{"query":{"match_all":{}}}"#),
        Some(r#"{"query":{"term":{"title":"brown"}},"size":5}"#),
    ] {
        let (status, text) = get(&node, &format!("/{EMPTY}/_search"), body);
        assert_eq!(status, 200, "{text}");
        assert!(text.contains(NO_HITS), "{text}");
        assert!(
            text.contains(r#""_shards":{"total":1,"successful":1,"skipped":0,"failed":0}"#),
            "{text}"
        );
    }
}

#[test]
fn a_count_answers_zero() {
    let node = node_with_empty_index();
    let (status, text) = get(&node, &format!("/{EMPTY}/_count"), None);
    assert_eq!(status, 200, "{text}");
    assert_eq!(
        text,
        r#"{"count":0,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0}}"#
    );
}

#[test]
fn a_search_with_an_aggregation_answers_empty_buckets() {
    let node = node_with_empty_index();
    let (status, text) = get(
        &node,
        &format!("/{EMPTY}/_search"),
        Some(r#"{"size":0,"aggs":{"n":{"value_count":{"field":"n"}}}}"#),
    );
    assert_eq!(status, 200, "{text}");
    assert!(
        text.contains(r#""aggregations":{"n":{"value":0}}"#),
        "{text}"
    );
}

#[test]
fn a_search_of_an_index_that_does_not_exist_is_still_404() {
    let node = node_with_empty_index();
    let (status, text) = get(&node, "/compat-no-such/_search", None);
    assert_eq!(status, 404, "{text}");
    assert!(text.contains("index_not_found_exception"), "{text}");
}

#[test]
fn the_reads_of_an_unwritten_index_make_no_list_and_no_write() {
    let node = node_with_empty_index();
    let before = counts(&node.store);
    for (target, body) in [
        ("_search", None),
        ("_count", None),
        ("_search", Some(r#"{"query":{"match_all":{}}}"#)),
    ] {
        let (status, text) = get(&node, &format!("/{EMPTY}/{target}"), body);
        assert_eq!(status, 200, "{text}");
    }
    let now = counts(&node.store);
    for op in [S3Op::List, S3Op::Head, S3Op::Put, S3Op::Copy, S3Op::Delete] {
        let i = S3Op::ALL.iter().position(|o| *o == op).expect("op");
        assert_eq!(now[i], before[i], "{op:?}");
    }
    let gets = |c: [u64; 7]| {
        S3Op::ALL
            .iter()
            .zip(c)
            .filter(|(o, _)| matches!(o, S3Op::Get | S3Op::GetRange))
            .map(|(_, n)| n)
            .sum::<u64>()
    };
    assert!(
        gets(now) - gets(before) <= 3,
        "one open: the descriptor, the register and the head, then none: {before:?} {now:?}"
    );
}
