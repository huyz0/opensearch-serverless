//! A `top_hits` under a keyword `terms`, run on the index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module. The
//! request parser still refuses the sub, so the `top_hits` is parsed at the
//! top level and moved under the `terms`.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_analysis::Analyzer;

use super::keyword_collect_tests::shard;
use crate::aggs::bucket::keyword_terms::KeywordTermsResult;
use crate::aggs::metric::TopHitsResult;
use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;
use crate::search::{Hit, SortValue, search};

const DOCS: usize = 200;

/// The `terms` result and the `top_hits` moved under it.
fn run(name: &str, terms: &str, top: &str) -> (KeywordTermsResult, Vec<TopHitsResult>) {
    let shard = shard(name, DOCS);
    let body = format!(r#"{{"aggs":{{"t":{terms},"th":{top}}}}}"#);
    let mut source = parse_chunks([body.as_bytes()]).expect("parses");
    let th = source.aggs.remove(1);
    source.aggs[0].subs.push(th);
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    let Value::KeywordTerms(t) = hits.aggregations.into_iter().next().expect("one").value else {
        panic!("not a keyword terms");
    };
    let tops = t
        .subs
        .iter()
        .map(|list| match &list[0] {
            Value::TopHits(top) => (**top).clone(),
            other => panic!("not a top_hits: {other:?}"),
        })
        .collect();
    (*t, tops)
}

fn docs(hits: &[Hit]) -> Vec<i32> {
    hits.iter().map(|h| h.doc).collect()
}

#[test]
fn each_bucket_holds_its_own_best_hits_in_the_sort() {
    let (t, tops) = run(
        "kw-th-sort",
        r#"{"terms":{"field":"tag","include":["t3","t4"]}}"#,
        r#"{"top_hits":{"size":2,"sort":[{"n":"desc"}]}}"#,
    );
    assert_eq!(t.buckets.len(), 2);
    for (i, (term, count)) in t.buckets.iter().enumerate() {
        let top = &tops[i];
        assert_eq!(top.total, *count, "{term}");
        assert_eq!(top.hits.len(), 2, "{term}");
        // n is the document's position, so the highest two of a bucket are its last.
        let want: Vec<i64> = {
            let n: i64 = term[1..].parse().expect("a number");
            // The bucket's n values are n, n + 16, ...; the largest two.
            let all: Vec<i64> = (0..13).map(|k| n + 16 * k).filter(|v| *v < 200).collect();
            all.iter().rev().take(2).copied().collect()
        };
        let got: Vec<i64> = top
            .hits
            .iter()
            .map(|h| match h.sort[0] {
                SortValue::Long(v) => v,
                ref other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, want, "{term}");
        assert!(top.before.is_empty());
    }
}

#[test]
fn from_keeps_the_hits_ahead_for_a_merge() {
    let (_, tops) = run(
        "kw-th-from",
        r#"{"terms":{"field":"tag","include":["t3"]}}"#,
        r#"{"top_hits":{"from":2,"size":1,"sort":[{"n":"desc"}]}}"#,
    );
    assert_eq!(tops[0].before.len(), 2);
    assert_eq!(tops[0].hits.len(), 1);
}

#[test]
fn a_top_hits_by_score_holds_scored_hits_in_each_bucket() {
    let (t, tops) = run(
        "kw-th-score",
        r#"{"terms":{"field":"tag","include":["t3"]}}"#,
        r#"{"top_hits":{"size":3}}"#,
    );
    assert_eq!(tops[0].total, t.buckets[0].1);
    assert_eq!(tops[0].hits.len(), 3);
    assert!(
        tops[0]
            .hits
            .iter()
            .all(|h| h.score.is_some() && h.sort.is_empty())
    );
    assert!(tops[0].max_score.is_some());
    // No two buckets share a hit.
    let (_, two) = run(
        "kw-th-score2",
        r#"{"terms":{"field":"tag","include":["t3","t4"]}}"#,
        r#"{"top_hits":{"size":3}}"#,
    );
    let a = docs(&two[0].hits);
    assert!(docs(&two[1].hits).iter().all(|d| !a.contains(d)));
}

#[test]
fn explain_and_highlight_under_a_bucket_are_refused() {
    let shard = shard("kw-th-refuse", DOCS);
    for top in [
        r#"{"top_hits":{"explain":true}}"#,
        r#"{"top_hits":{"highlight":{"fields":{"title":{}}}}}"#,
    ] {
        let body = format!(r#"{{"aggs":{{"t":{{"terms":{{"field":"tag"}}}},"th":{top}}}}}"#);
        let mut source = parse_chunks([body.as_bytes()]).expect("parses");
        let th = source.aggs.remove(1);
        source.aggs[0].subs.push(th);
        let err = search(&shard, "idx", &source, &Analyzer::standard(None)).expect_err("refused");
        assert_eq!(err.status, 400, "{top}");
    }
}

/// A `top_hits` moved under the first aggregation of `body`, and the first
/// aggregation's result.
fn first(name: &str, parent: &str) -> Value {
    let shard = shard(name, DOCS);
    let body = format!(
        r#"{{"aggs":{{"p":{parent},"th":{{"top_hits":{{"size":1,"sort":[{{"n":"desc"}}]}}}}}}}}"#
    );
    let mut source = parse_chunks([body.as_bytes()]).expect("parses");
    let th = source.aggs.remove(1);
    source.aggs[0].subs.push(th);
    search(&shard, "idx", &source, &Analyzer::standard(None))
        .expect("searches")
        .aggregations
        .remove(0)
        .value
}

/// The best `n` of each bucket's single hit.
fn bests(lists: &[Vec<Value>]) -> Vec<i64> {
    lists
        .iter()
        .map(|list| match &list[0] {
            Value::TopHits(top) => match top.hits[0].sort[0] {
                SortValue::Long(v) => v,
                ref other => panic!("{other:?}"),
            },
            other => panic!("not a top_hits: {other:?}"),
        })
        .collect()
}

#[test]
fn a_histogram_a_range_and_a_filters_hold_each_buckets_best_hit() {
    let Value::Histogram(h) = first("kw-th-hist", r#"{"histogram":{"field":"n","interval":50}}"#)
    else {
        panic!("not a histogram");
    };
    assert_eq!(bests(&h.subs.lists), [49, 99, 149, 199]);
    let Value::Range(r) = first(
        "kw-th-range",
        r#"{"range":{"field":"n","ranges":[{"to":100},{"from":100}]}}"#,
    ) else {
        panic!("not a range");
    };
    assert_eq!(bests(&r.subs), [99, 199]);
    let Value::Filters(f) = first(
        "kw-th-filters",
        r#"{"filters":{"filters":{"low":{"range":{"n":{"lt":10}}},"high":{"range":{"n":{"gte":190}}}}}}"#,
    ) else {
        panic!("not filters");
    };
    assert_eq!(bests(&f.subs), [199, 9]);
}

#[test]
fn the_shards_response_writes_each_buckets_hits() {
    let shard = shard("kw-th-write", DOCS);
    let body = r#"{"size":0,"aggs":{"t":{"terms":{"field":"tag","include":["t3","t4"]}},
        "th":{"top_hits":{"size":2,"sort":[{"n":"desc"}],"_source":false}}}}"#;
    let mut source = parse_chunks([body.as_bytes()]).expect("parses");
    let th = source.aggs.remove(1);
    source.aggs[0].subs.push(th);
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    let mut out = Vec::new();
    crate::response::write_response(&mut out, &shard, "idx", "node", &hits, 1, &mut |_| Ok(()))
        .expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    let agg = text
        .split_once(r#""aggregations":"#)
        .expect("aggregations")
        .1;
    // t3's best two n are 195 and 179; t4's are 196 and 180.
    for (key, first, second) in [("t3", 195, 179), ("t4", 196, 180)] {
        let bucket = agg
            .split(&format!(r#"{{"key":"{key}""#))
            .nth(1)
            .expect("bucket");
        assert!(
            bucket.contains(
                r#""th":{"hits":{"total":{"value":13,"relation":"eq"},"max_score":null,"hits":["#
            ),
            "{bucket}"
        );
        let sorts: Vec<&str> = bucket
            .split(r#""sort":["#)
            .skip(1)
            .take(2)
            .map(|s| s.split(']').next().expect("a value"))
            .collect();
        assert_eq!(sorts, [first.to_string(), second.to_string()], "{key}");
    }
}

#[test]
fn the_hits_a_bucket_holds_are_reserved_before_the_shard_collects_them() {
    let shard = shard("kw-th-reserve", DOCS);
    let reserved = |window: usize, docs: usize| {
        let body = format!(
            r#"{{"aggs":{{"t":{{"terms":{{"field":"tag"}}}},"th":{{"top_hits":{{"size":{window}}}}}}}}}"#
        );
        let mut source = parse_chunks([body.as_bytes()]).expect("parses");
        let th = source.aggs.remove(1);
        source.aggs[0].subs.push(th);
        crate::aggs::metric::top_hits_sub::reserved(&source.aggs, &shard, docs)
    };
    // Few documents bound the hits however large the window.
    assert!(reserved(1, DOCS) > 0);
    assert_eq!(reserved(1, DOCS), reserved(10, DOCS));
    // Many documents leave the window to bound them.
    assert!(reserved(10, 10_000_000) > reserved(1, 10_000_000));
    // No `top_hits` beneath, nothing reserved.
    let source =
        parse_chunks([br#"{"aggs":{"t":{"terms":{"field":"tag"}}}}"#.as_slice()]).expect("parses");
    assert_eq!(
        crate::aggs::metric::top_hits_sub::reserved(&source.aggs, &shard, DOCS),
        0
    );
}
