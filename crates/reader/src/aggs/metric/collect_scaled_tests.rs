//! Aggregations over a `scaled_float` field, on a real index of four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_blob::Emulator;
use oss_testkit::index::{SCALING_FACTOR, Spec, doc_scaled_long};

use super::tests::{DOCS, shard_of, values};
use crate::OpenShard;
use crate::aggs::bucket::composite::KeyPart;
use crate::aggs::metric::Value;

fn shard(name: &str) -> OpenShard<Emulator> {
    shard_of(Spec::new(name, DOCS).segments_of(50).with_decimals())
}

/// Document `i`'s scaled long, as the doc values read it: times the factor's inverse.
fn stored(i: usize) -> f64 {
    doc_scaled_long(i) as f64 * (1.0 / SCALING_FACTOR)
}

#[test]
fn metrics_of_a_scaled_float_multiply_each_long_by_the_inverse_factor() {
    let shard = shard("aggs-scaled");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"a":{"min":{"field":"s"}},"b":{"max":{"field":"s"}},
            "c":{"value_count":{"field":"s"}}}}"#,
    );
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
fn terms_and_histogram_of_a_scaled_float_key_by_the_scaled_double() {
    let shard = shard("aggs-scaled-buckets");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"t":{"terms":{"field":"s","size":2,"order":{"_key":"asc"}}}}}"#,
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
        r#"{"size":0,"aggs":{"h":{"histogram":{"field":"s","interval":5}}}}"#,
    );
    let Some(Value::Histogram(h)) = got.into_iter().next() else {
        panic!("not a histogram");
    };
    // Documents 0 to 40 hold 0.0 to 4.92: the first bucket.
    assert_eq!(h.buckets.first(), Some(&(0.0, 41)));
}

#[test]
fn a_composite_terms_source_on_a_scaled_float_keys_by_the_scaled_double() {
    let shard = shard("aggs-scaled-composite");
    let got = values(
        &shard,
        r#"{"size":0,"aggs":{"c":{"composite":{"size":2,"sources":[{"s":{"terms":{"field":"s"}}}]}}}}"#,
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
