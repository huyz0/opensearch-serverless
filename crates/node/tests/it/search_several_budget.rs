//! docs/specs/search-several-indexes: the shard bound of a list search, and what the read paths
//! added beside it cost in requests.

use oss_blob::S3Op;

use crate::search_several::{ALL, post, total};
use crate::support::{Node, counts, put_variant, request, start};

// bounds
/// A node holding `name`, a copy of `compat-term` whose descriptor says `shards` shards: only
/// shard 0 has blobs, which is all a plan reads.
fn with_declared_shards(name: &str, shards: u32) -> Node {
    let node = start();
    put_variant(&node.store, "compat-term", name, |j| {
        j.replace(
            r#""number_of_shards":1"#,
            &format!(r#""number_of_shards":{shards}"#),
        )
    });
    node
}

#[test]
fn a_list_visiting_more_shards_than_the_limit_is_refused_before_a_shard_is_read() {
    let node = with_declared_shards("wide", 1_024);
    // 1,024 shards and one: past the limit of 1,024.
    let (status, body) = post(&node, "/wide,compat-term/_search", ALL);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("at most 1024 shards"), "{body}");
    let refused = counts(&node.store);
    assert_eq!(post(&node, "/wide,compat-term/_search", ALL).0, 400);
    assert_eq!(counts(&node.store), refused, "a refusal reads nothing more");
    let (status, body) = post(&node, "/wide,compat-term/_count", "");
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("at most 1024 shards"), "{body}");
    let item = format!("{{\"index\":\"wide,compat-term\"}}\n{ALL}\n");
    let (status, answer) = post(&node, "/_msearch", &item);
    assert_eq!(status, 200, "{answer}");
    assert!(answer.contains(r#""status":400"#), "{answer}");
    assert!(answer.contains("at most 1024 shards"), "{answer}");
}

#[test]
fn the_shard_limit_counts_the_shards_visited_not_the_ones_declared() {
    let node = with_declared_shards("wide", 2_000);
    let routing =
        oss_cluster::routing::DocumentRouting::legacy_scaled(2_000, 1, 2_000).expect("layout");
    let value = (0..4_096)
        .map(|n| format!("r{n}"))
        .find(|r| routing.shard_for(r, Some(r)) == Ok(0))
        .expect("a value");
    let (_, one) = post(&node, "/compat-term/_search", ALL);
    let (status, body) = post(
        &node,
        &format!("/wide,compat-term/_search?routing={value}"),
        ALL,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(total(&body), 2 * total(&one), "{body}");
    let (status, body) = post(&node, "/wide,compat-term/_search", ALL);
    assert_eq!(status, 400, "{body}");
}

// budget of the read paths added after the list search
/// The requests `call` makes, by kind.
fn spent(node: &Node, call: impl FnOnce()) -> [u64; 7] {
    let before = counts(&node.store);
    call();
    let after = counts(&node.store);
    let mut out = [0u64; 7];
    for (i, n) in out.iter_mut().enumerate() {
        *n = after[i] - before[i];
    }
    out
}

fn lists_and_writes_nothing(spent: &[u64; 7], what: &str) {
    for (i, op) in S3Op::ALL.iter().enumerate() {
        if matches!(op, S3Op::List | S3Op::Put | S3Op::Copy) {
            assert_eq!(spent[i], 0, "{what}: {op:?} in {spent:?}");
        }
    }
}

#[test]
fn an_msearch_item_costs_the_same_each_time_and_its_parameters_cost_nothing() {
    let node = start();
    let run = |query: &str, header: &str| {
        let body = format!("{header}\n{ALL}\n");
        let r = request(
            node.addr(),
            "POST",
            &format!("/_msearch{query}"),
            Some(body.as_bytes()),
        );
        assert_eq!(r.status, 200, "{}", r.text());
    };
    let header = r#"{"index":"compat-term,no-such-index","ignore_unavailable":true}"#;
    let cold = spent(&node, || run("", header));
    lists_and_writes_nothing(&cold, "cold");
    let warm = spent(&node, || run("", header));
    assert_eq!(
        spent(&node, || run("", header)),
        warm,
        "a repeat costs the same"
    );
    let with = spent(&node, || {
        run(
            "?rest_total_hits_as_int=true&max_concurrent_searches=2",
            header,
        );
    });
    assert_eq!(with, warm, "the parameters read nothing");
    lists_and_writes_nothing(&warm, "warm");
}

#[test]
fn field_caps_over_a_list_costs_the_same_each_time_and_lists_nothing() {
    let node = start();
    let path =
        "/compat-term,compat-bool,no-such-index/_field_caps?fields=*&ignore_unavailable=true";
    let run = || {
        let r = request(node.addr(), "GET", path, None);
        assert_eq!(r.status, 200, "{}", r.text());
    };
    let cold = spent(&node, run);
    lists_and_writes_nothing(&cold, "cold");
    let warm = spent(&node, run);
    assert_eq!(spent(&node, run), warm, "a repeat costs the same");
    lists_and_writes_nothing(&warm, "warm");
    // Each index is read once however often it is listed.
    let twice = "/compat-term,compat-bool,compat-term,no-such-index/_field_caps?fields=*&ignore_unavailable=true";
    let doubled = spent(&node, || {
        assert_eq!(request(node.addr(), "GET", twice, None).status, 200);
    });
    assert_eq!(doubled, warm, "a name listed twice is read once");
}
