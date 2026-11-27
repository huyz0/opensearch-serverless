//! Aggregations over a `half_float` field, on a real index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_util::half_float::{half_float_to_sortable_short, sortable_short_to_half_float};
use oss_blob::Emulator;
use oss_testkit::index::{Spec, doc_half};

use super::tests::{DOCS, shard_of, values};
use crate::OpenShard;
use crate::aggs::bucket::composite::KeyPart;
use crate::aggs::metric::Value;

fn shard(name: &str) -> OpenShard<Emulator> {
    shard_of(Spec::new(name, DOCS).segments_of(50).with_decimals())
}

/// Document `i`'s half, widened to a double as the doc values read it.
fn stored(i: usize) -> f64 {
    f64::from(sortable_short_to_half_float(half_float_to_sortable_short(
        doc_half(i),
    )))
}

#[test]
fn metrics_of_a_half_float_widen_each_stored_half_to_a_double() {
    let shard = shard("aggs-half");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"a":{"min":{"field":"h"}},"b":{"max":{"field":"h"}},
            "c":{"value_count":{"field":"h"}}}}"#,
    );
    // 19.9 is held as the half nearest it, not as the float or the double.
    assert_ne!(stored(199), 19.9);
    assert_eq!(
        got,
        [
            Value::Min(0.0),
            Value::Max(stored(199)),
            Value::ValueCount(200)
        ]
    );
}

#[test]
fn terms_and_histogram_of_a_half_float_key_by_the_widened_double() {
    let shard = shard("aggs-half-buckets");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"h","size":2,"order":{"_key":"asc"}}}}}"#,
    );
    let Some(Value::Terms(terms)) = got.into_iter().next() else {
        panic!("not a terms");
    };
    let mut out = Vec::new();
    crate::aggs::bucket::terms_render::write_terms(
        &mut out,
        crate::aggs::partial::Format::Double,
        &terms,
    )
    .expect("writes");
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        format!(
            r#""doc_count_error_upper_bound":0,"sum_other_doc_count":198,"buckets":[{{"key":0.0,"doc_count":1}},{{"key":{:?},"doc_count":1}}]"#,
            stored(1)
        )
    );
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"h":{"histogram":{"field":"h","interval":5}}}}"#,
    );
    let Some(Value::Histogram(h)) = got.into_iter().next() else {
        panic!("not a histogram");
    };
    assert_eq!(h.buckets.first(), Some(&(0.0, 50)));
}

#[test]
fn a_composite_terms_source_on_a_half_float_keys_by_the_widened_double() {
    let shard = shard("aggs-half-composite");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"c":{"composite":{"size":2,"sources":[{"s":{"terms":{"field":"h"}}}]}}}}"#,
    );
    let Some(Value::Composite(c)) = got.into_iter().next() else {
        panic!("not a composite");
    };
    let keys: Vec<_> = c
        .buckets
        .into_iter()
        .map(|(k, n)| (k[0].clone(), n))
        .collect();
    assert_eq!(
        keys,
        [(KeyPart::Double(0.0), 1), (KeyPart::Double(stored(1)), 1)]
    );
}
