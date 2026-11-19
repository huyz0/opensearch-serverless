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
