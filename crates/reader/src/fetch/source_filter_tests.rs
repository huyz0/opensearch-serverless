//! Tests of `_source` filtering; compiled only under `#[cfg(test)]`. The
//! expected sources were answered by stock OpenSearch 3.10.0 for the same
//! document and `_source`.

use super::*;
use crate::fetch::source_map::{java_hash_order, spread, write_number};

const DOC: &str = r#"{"b":1,"a":{"y":"x","x":[1,{"q":2,"p":3},[4,{"r":5}]],"z.w":1.5},"title":"Quick é \"q\"","n":-0,"f":1e2,"big":12345678.5,"tiny":0.0001,"arr":[{"k":1},{"m":2}],"dotted.key":true,"nil":null,"BB":2,"Aa":3,"z":{"inner":{"deep":1}}}"#;

fn filter(source: &str) -> Result<SourceFilter, String> {
    let body = format!(r#"{{"_source":{source}}}"#);
    let mut rec = Recorder::new(1 << 16);
    let mut tokenizer = crate::dsl::source::tokenizer();
    tokenizer.feed(body.as_bytes(), &mut rec).expect("tokens");
    tokenizer.finish(&mut rec).expect("tokens");
    let mut p = rec.parser();
    p.next_token();
    p.next_token();
    p.next_token();
    parse_source(&mut p, "_source").map_err(|e| e.into_failure().reason)
}

fn filtered(source: &str) -> String {
    let f = filter(source).expect("parses");
    let mut out = Vec::new();
    write_filtered(&mut out, &f, DOC.as_bytes()).expect("writes");
    String::from_utf8(out).expect("utf8")
}

#[test]
fn each_filter_writes_what_opensearch_wrote() {
    for (source, expected) in [
        (r#"["title"]"#, r#"{"title":"Quick é \"q\""}"#),
        (
            r#"["a","b"]"#,
            r#"{"a":{"z.w":1.5,"x":[1,{"p":3,"q":2},[4,{"r":5}]],"y":"x"},"b":1}"#,
        ),
        (r#"["a.x"]"#, r#"{"a":{"x":[1,{"p":3,"q":2},[4,{"r":5}]]}}"#),
        (
            r#"["a.*"]"#,
            r#"{"a":{"z.w":1.5,"x":[1,{"p":3,"q":2},[4,{"r":5}]],"y":"x"}}"#,
        ),
        (
            r#"{"excludes":["a"]}"#,
            r#"{"arr":[{"k":1},{"m":2}],"BB":2,"Aa":3,"nil":null,"big":1.23456785E7,"b":1,"dotted.key":true,"f":100.0,"tiny":1.0E-4,"z":{"inner":{"deep":1}},"title":"Quick é \"q\"","n":0}"#,
        ),
        (
            r#"{"includes":["a"],"excludes":["a.x"]}"#,
            r#"{"a":{"z.w":1.5,"y":"x"}}"#,
        ),
        (r#"["*.q"]"#, r#"{"a":{"x":[{"q":2}]}}"#),
        (r#"["arr.k"]"#, r#"{"arr":[{"k":1}]}"#),
        (r#"["a.z.w"]"#, r#"{"a":{"z.w":1.5}}"#),
        (
            r#"{"excludes":["*.deep"]}"#,
            r#"{"arr":[{"k":1},{"m":2}],"BB":2,"Aa":3,"a":{"z.w":1.5,"x":[1,{"p":3,"q":2},[4,{"r":5}]],"y":"x"},"big":1.23456785E7,"b":1,"f":100.0,"title":"Quick é \"q\"","n":0,"nil":null,"dotted.key":true,"tiny":1.0E-4,"z":{"inner":{}}}"#,
        ),
        (r#"["dotted"]"#, r#"{"dotted.key":true}"#),
        (r#"["z"]"#, r#"{"z":{"inner":{"deep":1}}}"#),
        (
            r#"{"includes":["n","f","big","tiny","nil","title"]}"#,
            r#"{"nil":null,"big":1.23456785E7,"f":100.0,"tiny":1.0E-4,"title":"Quick é \"q\"","n":0}"#,
        ),
        (r#"["x*"]"#, r#"{}"#),
    ] {
        assert_eq!(filtered(source), expected, "{source}");
    }
}

#[test]
fn whole_sources_and_none_are_not_filtered() {
    for whole in ["true", "[]", "{}", r#"{"includes":[]}"#] {
        assert!(filter(whole).expect("parses").is_whole(), "{whole}");
    }
    let none = filter("false").expect("parses");
    assert!(none.skip && !none.is_whole());
    assert_eq!(
        filter(r#"{"include":"a","exclude":["b","b"]}"#),
        Ok(SourceFilter {
            skip: false,
            includes: vec!["a".into()],
            excludes: vec!["b".into()],
        })
    );
}

#[test]
fn bad_filters_fail_as_opensearch_fails() {
    assert_eq!(
        filter(r#"{"includes":"a","excludes":"a"}"#),
        Err("The same entry [a] cannot be both included and excluded in _source.".into())
    );
    assert_eq!(
        filter("1"),
        Err(
            "Expected one of [VALUE_BOOLEAN, VALUE_STRING, START_ARRAY, START_OBJECT] but found \
             [VALUE_NUMBER]"
                .into()
        )
    );
    assert_eq!(
        filter(r#"{"include":["b"],"x":["c"]}"#),
        Err("Unknown key for a START_ARRAY in [x].".into())
    );
    assert_eq!(
        filter(r#"{"include":["a",true]}"#),
        Err("Unknown key for a VALUE_BOOLEAN in [null].".into())
    );
    assert_eq!(
        filter(r#"{"include":1}"#),
        Err("Unknown key for a VALUE_NUMBER in [include].".into())
    );
    assert_eq!(
        filter("[1]"),
        Err("Unknown key for a VALUE_NUMBER in [null].".into())
    );
}

#[test]
fn keys_iterate_in_java_hash_map_order() {
    // "Aa" and "BB" share a hash; the first put comes first.
    let mut order = Vec::new();
    java_hash_order(&["BB", "x", "Aa"], &mut order);
    assert_eq!(order, vec![0, 2, 1]);
    // The same order as the aggregations' model, which also resizes a
    // table under 64 for a ninth key in one bucket.
    let names: Vec<String> = (0..200).map(|i| format!("k{}", i * 16)).collect();
    let keys: Vec<&str> = names.iter().map(String::as_str).collect();
    for n in [0, 1, 12, 13, 40, 49, 97, 200] {
        let some = keys.get(..n).unwrap_or_default();
        java_hash_order(some, &mut order);
        assert_eq!(
            order,
            crate::aggs::metric::hash_order::hash_map_order(some),
            "{n}"
        );
    } // Nine keys in one bucket of 16 resize the table before it is full.
    let names: Vec<String> = (0..2000)
        .map(|i| format!("f{i}"))
        .filter(|k| spread(k) & 15 == 0)
        .take(12)
        .collect();
    let keys: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(keys.len(), 12);
    java_hash_order(&keys, &mut order);
    assert_eq!(
        order,
        crate::aggs::metric::hash_order::hash_map_order(&keys)
    );
    assert_ne!(order, (0..12).collect::<Vec<_>>());
}

#[test]
fn numbers_print_as_java_prints_them() {
    let w = |t: &str| {
        let mut out = Vec::new();
        write_number(&mut out, t).expect("writes");
        String::from_utf8(out).expect("utf8")
    };
    assert_eq!(w("-0"), "0");
    assert_eq!(w("42"), "42");
    assert_eq!(w("-7"), "-7");
    assert_eq!(w("1e2"), "100.0");
    assert_eq!(w("1.5"), "1.5");
    assert_eq!(w("12345678.5"), "1.23456785E7");
    assert_eq!(w("0.0001"), "1.0E-4");
    assert_eq!(w("1E300"), "1.0E300");
    assert_eq!(w("-0.0"), "-0.0");
}

#[test]
fn a_source_that_is_not_an_object_is_an_error() {
    let f = filter(r#"["a"]"#).expect("parses");
    assert!(write_filtered(&mut Vec::new(), &f, b"[1]").is_err());
    assert!(write_filtered(&mut Vec::new(), &f, b"{").is_err());
}

#[test]
fn a_full_bucket_of_a_table_of_64_does_not_grow_it() {
    // 25 keys grow the table to 64; nine more in one of its buckets, split
    // by the next bit, make Java treeify that bucket rather than resize, so
    // the keys still come out by bucket of 64.
    let mut names: Vec<String> = (0..25).map(|i| format!("g{i}")).collect();
    names.extend(
        (0..5000)
            .map(|i| format!("c{i}"))
            .filter(|k| spread(k) & 63 == 5)
            .take(9),
    );
    let keys: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(keys.len(), 34);
    assert!(keys.iter().skip(25).any(|k| spread(k) & 64 != 0));
    let mut order = Vec::new();
    java_hash_order(&keys, &mut order);
    let buckets: Vec<u32> = order.iter().map(|&i| spread(keys[i]) & 63).collect();
    assert!(buckets.windows(2).all(|w| w[0] <= w[1]), "{buckets:?}");
}

#[test]
fn deeper_paths_and_nested_arrays_filter_as_opensearch_filters() {
    // Each expected source was written by OpenSearch 3.10.0's
    // `XContentMapValues.filter` for the same source and lists.
    let run = |source: &str, includes: &[&str], excludes: &[&str]| {
        let f = SourceFilter {
            skip: false,
            includes: includes.iter().map(|s| (*s).to_owned()).collect(),
            excludes: excludes.iter().map(|s| (*s).to_owned()).collect(),
        };
        let mut out = Vec::new();
        write_filtered(&mut out, &f, source.as_bytes()).expect("writes");
        String::from_utf8(out).expect("utf8")
    };
    // A key that starts with a dot is matched whole by the set filter.
    assert_eq!(run(r#"{".x":1,"x.y":2,"y":3}"#, &[""], &[]), "{}");
    // A path that ends at an object keeps the object whole.
    assert_eq!(
        run(r#"{"a":{"b":{"c":1},"d":2}}"#, &["a.b"], &[]),
        r#"{"a":{"b":{"c":1}}}"#
    );
    // A dotted key whose first part a pattern names is in it.
    assert_eq!(
        run(r#"{"a.b":1,"c":2}"#, &["a", "q.r"], &[]),
        r#"{"a.b":1}"#
    );
    assert_eq!(run(r#"{"a.b":1,"c":2}"#, &[], &["a", "q.r"]), r#"{"c":2}"#);
    // Arrays inside arrays are filtered as their parent is.
    let nested = r#"{"a":[[1,2],[{"b":1,"c":2}],3]}"#;
    assert_eq!(run(nested, &["a.b"], &[]), r#"{"a":[[{"b":1}]]}"#);
    assert_eq!(
        run(nested, &["a*"], &["a.c"]),
        r#"{"a":[[1,2],[{"b":1}],3]}"#
    );
}
