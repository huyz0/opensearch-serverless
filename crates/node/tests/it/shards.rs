//! docs/specs/node-multi-shard-open: a shard past the first opens, is kept
//! and takes a pushed commit.

use oss_blob::S3Op;

use crate::support::{counts, put_shard_copy, put_variant, start};

const UUID: &str = "compat-term-uuid";

fn two_shards() -> crate::support::Node {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", UUID, 1);
    node
}

fn gets(node: &crate::support::Node) -> u64 {
    counts(&node.store)[S3Op::ALL.iter().position(|o| *o == S3Op::Get).expect("get")]
}

#[test]
fn a_second_shard_opens_with_two_gets_and_is_kept_beside_the_first() {
    let node = two_shards();
    let before = gets(&node);
    node.service.open("two", 1).expect("opens");
    assert_eq!(gets(&node) - before, 2, "the descriptor and the commit");
    assert_eq!(node.service.open_indexes(), 1);
    node.service.open("two", 0).expect("opens");
    assert_eq!(node.service.open_indexes(), 2, "a shard each");
    let warm = gets(&node);
    node.service.open("two", 1).expect("kept");
    node.service.open("two", 0).expect("kept");
    assert_eq!(gets(&node), warm, "open shards are not read again");
    assert_eq!(node.service.open_indexes(), 2);
}

#[test]
fn a_shard_the_index_does_not_have_is_refused_and_opens_nothing() {
    let node = two_shards();
    let e = node.service.open("two", 2).expect_err("out of range");
    assert_eq!(e.status, 400);
    let body = String::from_utf8(e.to_body()).expect("utf-8");
    assert!(
        body.contains("shard [2] is out of range for index [two]"),
        "{body}"
    );
    assert_eq!(node.service.open_indexes(), 0);
    let e = node.service.open("missing", 0).expect_err("no index");
    assert_eq!(e.status, 404);
}

#[test]
fn a_push_for_a_shard_past_the_first_reaches_that_shard_alone() {
    let node = two_shards();
    node.service.open("two", 0).expect("opens");
    assert!(
        !node.service.take_push("two", UUID, 1, b""),
        "shard 1 is not open here"
    );
    node.service.open("two", 1).expect("opens");
    assert!(node.service.take_push("two", UUID, 1, b""));
    assert!(
        !node.service.take_push("two", "another-uuid", 1, b""),
        "another index of the name"
    );
    assert_eq!(node.service.open_indexes(), 2);
}

// ---- docs/specs/node-shard-query

use oss_reader::dsl::{SearchSource, parse_chunks};

use crate::support::{NODE, request};

fn source(body: &str) -> SearchSource {
    parse_chunks([body.as_bytes()]).expect("parses")
}

/// The hit list of a search response body.
fn hit_list(body: &str) -> &str {
    let from = body.find(r#""hits":["#).expect("hits") + 8;
    let end = body[from..].rfind("]}").expect("end") + from;
    &body[from..end]
}

const TERM: &str = r#"{"query":{"term":{"title":"brown"}}}"#;

#[test]
fn a_shard_query_finds_what_the_http_search_finds_and_writes_its_hits_the_same() {
    let node = start();
    let q = node
        .service
        .query_shard("compat-term", 0, &source(TERM))
        .expect("queries");
    assert_eq!((q.hits.total, q.hits.hits.len()), (2, 2));
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(TERM.as_bytes()),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let written = q.render(NODE, &[0, 1], &mut |_| Ok(())).expect("renders");
    let joined: Vec<String> = written
        .into_iter()
        .map(|h| String::from_utf8(h).expect("utf-8"))
        .collect();
    assert_eq!(joined.join(","), hit_list(&r.text()));
    let back = q.render(NODE, &[1, 0], &mut |_| Ok(())).expect("renders");
    assert_eq!(back[0], joined[1].as_bytes());
}

#[test]
fn a_shard_past_the_first_is_queried_and_its_hits_name_it() {
    let node = two_shards();
    let explained = r#"{"query":{"term":{"title":"brown"}},"explain":true}"#;
    let first = node
        .service
        .query_shard("two", 0, &source(explained))
        .expect("queries");
    let second = node
        .service
        .query_shard("two", 1, &source(explained))
        .expect("queries");
    assert_eq!(first.hits.total, second.hits.total, "the same data twice");
    let one = |q: &oss_node::search::ShardQuery<_>| {
        String::from_utf8(
            q.render(NODE, &[0], &mut |_| Ok(()))
                .expect("renders")
                .remove(0),
        )
        .expect("utf-8")
    };
    assert!(
        one(&first).starts_with(r#"{"_shard":"[two][0]""#),
        "{}",
        one(&first)
    );
    assert!(
        one(&second).starts_with(r#"{"_shard":"[two][1]""#),
        "{}",
        one(&second)
    );
}

#[test]
fn a_failed_shard_query_names_the_shard() {
    let node = two_shards();
    let e = node
        .service
        .query_shard("two", 1, &source(r#"{"from":9999,"size":10}"#))
        .err()
        .expect("too large a window");
    assert_eq!(e.status, 400);
    let body = String::from_utf8(e.to_body()).expect("utf-8");
    assert!(
        body.contains(r#""shard":1"#) && body.contains(r#""index":"two""#),
        "{body}"
    );
    let e = node
        .service
        .query_shard("missing", 0, &source("{}"))
        .err()
        .expect("no index");
    assert_eq!(e.status, 404);
    assert!(node.service.query_shard("two", 2, &source("{}")).is_err());
}
