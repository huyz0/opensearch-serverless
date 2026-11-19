//! docs/specs/node-coordinate: a search over an index's shards on one node.

use std::io;

use oss_reader::dsl::parse_chunks;

use crate::support::{Node, put_shard_copy, put_variant, start};

fn two(index: &str, uuid: &str, name: &str) -> Node {
    let node = start();
    put_variant(&node.store, index, name, |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, index, uuid, 1);
    node
}

fn run(
    node: &Node,
    index: &str,
    shards: usize,
    body: &str,
    reserve: &(dyn Fn(usize) -> io::Result<()> + Sync),
) -> Result<String, String> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    node.service
        .coordinate(index, shards, &source, 3, reserve)
        .map(|b| String::from_utf8(b).expect("utf-8"))
        .map_err(|e| format!("{e:?}"))
}

fn grant(_: usize) -> io::Result<()> {
    Ok(())
}

#[test]
fn two_shards_answer_as_one_with_each_hit_from_its_shard() {
    let node = two("compat-term", "compat-term-uuid", "two");
    let one = run(
        &node,
        "two",
        1,
        r#"{"query":{"term":{"title":"brown"}}}"#,
        &grant,
    )
    .expect("one shard");
    assert!(
        one.contains(r#""total":{"value":2,"relation":"eq"}"#),
        "{one}"
    );
    let both = run(
        &node,
        "two",
        2,
        r#"{"query":{"term":{"title":"brown"}}}"#,
        &grant,
    )
    .expect("two shards");
    assert!(
        both.contains(r#""_shards":{"total":2,"successful":2,"skipped":0,"failed":0}"#),
        "{both}"
    );
    assert!(
        both.contains(r#""total":{"value":4,"relation":"eq"}"#),
        "{both}"
    );
    assert_eq!(
        both.matches(r#""_id":"4""#).count(),
        2,
        "one hit from each shard: {both}"
    );
    let paged = run(&node, "two", 2, r#"{"from":4,"size":3}"#, &grant).expect("paged");
    assert_eq!(paged.matches(r#""_id":"#).count(), 3, "{paged}");
}

#[test]
fn the_sort_orders_the_merge() {
    let node = two("compat-paging-sort", "compat-paging-sort-uuid", "sorted");
    let body = run(
        &node,
        "sorted",
        2,
        r#"{"size":100,"sort":[{"year":"desc"},{"_doc":"asc"}]}"#,
        &grant,
    )
    .expect("searches");
    let years: Vec<i64> = body
        .split(r#""sort":["#)
        .skip(1)
        .filter_map(|rest| rest.split([',', ']']).next()?.parse().ok())
        .collect();
    assert!(years.len() >= 4, "{body}");
    assert!(years.windows(2).all(|w| w[0] >= w[1]), "{years:?}");
}

#[test]
fn a_shard_the_index_lacks_fails_alone() {
    let node = two("compat-term", "compat-term-uuid", "two");
    let body = run(&node, "two", 3, r#"{"size":100}"#, &grant).expect("still answers");
    assert!(
        body.contains(r#""total":3,"successful":2,"skipped":0,"failed":1,"failures":[{"shard":2,"index":"two""#),
        "{body}"
    );
    assert_eq!(body.matches(r#""_id":"1""#).count(), 2, "{body}");
}

#[test]
fn a_refused_reserve_fails_the_search() {
    let node = two("compat-term", "compat-term-uuid", "two");
    // The two shards' own reservations go through; the render's is refused.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let refuse = |_: usize| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
            return Ok(());
        }
        Err(io::Error::other("the request-body pool is full"))
    };
    let e = run(&node, "two", 2, r#"{"size":10}"#, &refuse).expect_err("refused");
    assert!(e.contains("Fetch"), "{e}");
    assert!(e.contains("the request-body pool is full"), "{e}");
}

#[test]
fn each_shard_reserves_its_own_hits_before_it_searches() {
    let node = two("compat-term", "compat-term-uuid", "two");
    let seen = std::sync::Mutex::new(Vec::new());
    let record = |n: usize| {
        seen.lock().expect("lock").push(n);
        Ok(())
    };
    run(&node, "two", 2, r#"{"size":3}"#, &record).expect("searches");
    let seen = seen.into_inner().expect("lock");
    assert!(seen.len() >= 2, "{seen:?}");
    assert!(seen[..2].iter().all(|n| *n > 0), "{seen:?}");
}

#[test]
fn shards_whose_hits_do_not_fit_fail_before_they_search() {
    let node = two("compat-term", "compat-term-uuid", "two");
    let refuse = |_: usize| Err(io::Error::other("the request-body pool is full"));
    let e = run(&node, "two", 2, r#"{"size":3}"#, &refuse).expect_err("refused");
    assert!(e.contains("AllFailed"), "{e}");
    assert!(e.contains("pool is full"), "{e}");
}

#[test]
fn the_total_follows_track_total_hits() {
    let node = two("compat-term", "compat-term-uuid", "two");
    let off = run(&node, "two", 2, r#"{"track_total_hits":false}"#, &grant).expect("searches");
    assert!(!off.contains(r#""total":{"#), "{off}");
    let capped = run(&node, "two", 2, r#"{"track_total_hits":2}"#, &grant).expect("searches");
    assert!(
        capped.contains(r#""total":{"value":2,"relation":"gte"}"#),
        "{capped}"
    );
}

#[test]
fn the_search_route_answers_an_index_of_two_shards_through_the_coordinator() {
    let node = two("compat-term", "compat-term-uuid", "two");
    let before = crate::support::counts(&node.store);
    let r = crate::support::request(
        node.addr(),
        "POST",
        "/two/_search",
        Some(br#"{"query":{"term":{"title":"brown"}}}"#),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.text();
    assert!(
        body.contains(r#""_shards":{"total":2,"successful":2,"skipped":0,"failed":0}"#),
        "{body}"
    );
    assert!(
        body.contains(r#""total":{"value":4,"relation":"eq"}"#),
        "{body}"
    );
    let again = crate::support::counts(&node.store);
    let r = crate::support::request(node.addr(), "POST", "/two/_search", Some(b"{}"));
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        crate::support::counts(&node.store),
        again,
        "a warm search reads nothing"
    );
    assert!(again.iter().sum::<u64>() > before.iter().sum::<u64>());
}
