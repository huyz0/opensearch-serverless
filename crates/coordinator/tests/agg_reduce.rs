//! docs/specs/coordinator-agg-reduce: each named aggregation reduces across shards.

use oss_coordinator::reduce::aggs::{AggsError, reduce_shards};
use oss_reader::aggs::metric::top_hits::TopHitsResult;
use oss_reader::aggs::partial::Value;

fn named(name: &str, value: Value) -> (String, Value) {
    (name.to_string(), value)
}

#[test]
fn a_name_every_shard_answered_reduces_over_all_of_them() {
    let shards = vec![
        vec![named("s", Value::Sum(1.5)), named("lo", Value::Min(4.0))],
        vec![named("s", Value::Sum(2.0)), named("lo", Value::Min(3.0))],
    ];
    let out = reduce_shards(&shards).expect("reduces");
    assert_eq!(
        out,
        vec![named("s", Value::Sum(3.5)), named("lo", Value::Min(3.0))]
    );
}

#[test]
fn a_name_only_some_shards_have_reduces_over_those() {
    let shards = vec![
        vec![named("a", Value::ValueCount(2))],
        vec![named("b", Value::Max(9.0))],
        vec![named("a", Value::ValueCount(5))],
    ];
    let out = reduce_shards(&shards).expect("reduces");
    assert_eq!(
        out,
        vec![
            named("a", Value::ValueCount(7)),
            named("b", Value::Max(9.0))
        ]
    );
}

#[test]
fn names_come_out_once_in_first_seen_order() {
    let shards = vec![
        vec![named("z", Value::Sum(1.0))],
        vec![named("a", Value::Sum(1.0)), named("z", Value::Sum(1.0))],
    ];
    let out = reduce_shards(&shards).expect("reduces");
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["z", "a"]);
}

#[test]
fn values_of_different_kinds_are_refused() {
    let shards = vec![
        vec![named("x", Value::Sum(1.0))],
        vec![named("x", Value::Min(1.0))],
    ];
    assert_eq!(
        reduce_shards(&shards),
        Err(AggsError::Mismatched { name: "x".into() })
    );
}

#[test]
fn top_hits_is_refused_for_the_piece_that_fetches_hits() {
    let shards = vec![vec![named(
        "t",
        Value::TopHits(Box::new(TopHitsResult::empty())),
    )]];
    assert_eq!(
        reduce_shards(&shards),
        Err(AggsError::TopHits { name: "t".into() })
    );
}

#[test]
fn no_shards_or_no_aggregations_give_none() {
    assert_eq!(reduce_shards(&[]), Ok(Vec::new()));
    assert_eq!(reduce_shards(&[Vec::new(), Vec::new()]), Ok(Vec::new()));
}
