//! `percentiles` and `percentile_ranks` parsed, written out and reduced.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_aggs::tdigest::MergingDigest;

use super::*;
use crate::aggs::metric::{tdigest, write_aggregations};
use crate::aggs::partial::{Partial, Value, reduce};
use crate::dsl::{SearchSource, parse_chunks};

fn source(body: &str) -> Result<SearchSource, DslError> {
    parse_chunks([body.as_bytes()])
}

/// The options of `ty` (`percentiles` or `percentile_ranks`) with `inner`
/// after its field.
fn options(ty: &str, inner: &str) -> PercentilesOptions {
    let body = format!(r#"{{"aggs":{{"p":{{"{ty}":{{"field":"n"{inner}}}}}}}}}"#);
    let s = source(&body).expect("parses");
    assert_eq!(s.aggs[0].field, "n");
    s.aggs[0].percentiles.clone()
}

/// The failure's reason and its causes', outermost first.
fn refused(ty: &str, inner: &str) -> Vec<String> {
    let body = format!(r#"{{"aggs":{{"p":{{"{ty}":{{{inner}}}}}}}}}"#);
    let f = source(&body).expect_err("refused").into_failure();
    let mut reasons = vec![f.reason.clone()];
    let mut cause = f.cause.as_deref();
    while let Some(c) = cause {
        reasons.push(c.reason.clone());
        cause = c.cause.as_deref();
    }
    reasons
}

#[test]
fn java_reads_doubles_as_parse_double_reads_them() {
    for (text, want) in [
        ("5", Some(5.0)),
        (" 5 ", Some(5.0)),
        ("\t7d", Some(7.0)),
        ("1e3f", Some(1000.0)),
        ("2.D", Some(2.0)),
        ("-0.5", Some(-0.5)),
        ("+3", Some(3.0)),
        ("Infinity", Some(f64::INFINITY)),
        ("-Infinity", Some(f64::NEG_INFINITY)),
        ("1e400", Some(f64::INFINITY)),
    ] {
        assert_eq!(java_parse_double(text), want, "{text:?}");
    }
    assert!(java_parse_double("NaN").is_some_and(f64::is_nan));
    assert!(java_parse_double("-NaN").is_some_and(f64::is_nan));
    for text in [
        "", "+", "d", "x", "inf", "nan", "infinity", "0x1p3", "1,5", "5 5", "e",
    ] {
        assert_eq!(java_parse_double(text), None, "{text:?}");
    }
}

#[test]
fn options_parse_as_opensearch_reads_them() {
    let defaults = options("percentiles", "");
    assert_eq!(defaults.keys, DEFAULT_PERCENTS);
    assert!(defaults.keyed);
    assert_eq!(defaults.compression, DEFAULT_COMPRESSION);
    let set = options(
        "percentiles",
        r#","percents":[50,"0.5",50,-0.0,0,100],"keyed":"false","tdigest":{"compression":"5"}"#,
    );
    // `Arrays.sort`: -0.0 before 0.0, repeats kept.
    assert_eq!(set.keys.len(), 6);
    assert!(set.keys[0].is_sign_negative() && set.keys[0] == 0.0);
    assert!(set.keys[1].is_sign_positive() && set.keys[1] == 0.0);
    assert_eq!(set.keys[2..], [0.5, 50.0, 50.0, 100.0]);
    assert!(!set.keyed);
    assert_eq!(set.compression, 5.0);
    assert_eq!(options("percentiles", r#","percents":"75""#).keys, [75.0]);
    assert_eq!(options("percentiles", r#","percents":25"#).keys, [25.0]);
    assert!(!options("percentiles", r#","keyed":false"#).keyed);
    assert!(options("percentiles", r#","keyed":"true""#).keyed);
    assert_eq!(
        options("percentiles", r#","tdigest":{}"#).compression,
        100.0
    );
    assert_eq!(
        options("percentiles", r#","tdigest":{"compression":0}"#).compression,
        0.0
    );
    // NaN sorts last, and is a percent OpenSearch accepts.
    let nan = options("percentiles", r#","percents":["NaN",3]"#);
    assert_eq!(nan.keys[0], 3.0);
    assert!(nan.keys[1].is_nan());
    let ranks = options("percentile_ranks", r#","values":[3,1e12,-2]"#);
    assert_eq!(ranks.keys, [-2.0, 3.0, 1e12]);
    // `field` may come after the keys.
    let late =
        source(r#"{"aggs":{"p":{"percentiles":{"percents":[9],"field":"m"}}}}"#).expect("parses");
    assert_eq!(late.aggs[0].field, "m");
    assert_eq!(late.aggs[0].metric, Metric::Percentiles);
}

#[test]
fn malformed_options_fail_as_opensearch_fails_them() {
    let build = |ty: &str, why: &str| {
        vec![
            format!("Failed to build [{ty}] after last required field arrived"),
            why.to_owned(),
        ]
    };
    for (ty, inner, want) in [
        (
            "percentiles",
            r#""field":"n","percents":[]"#,
            build("percentiles", "[percents] must not be empty: [p]"),
        ),
        (
            "percentiles",
            r#""field":"n","percents":[5,101]"#,
            build(
                "percentiles",
                "percent must be in [0,100], got [101.0]: [p]",
            ),
        ),
        (
            "percentiles",
            r#""field":"n","percents":[-1e-3]"#,
            build(
                "percentiles",
                "percent must be in [0,100], got [-0.001]: [p]",
            ),
        ),
        (
            "percentile_ranks",
            r#""field":"n""#,
            build("percentile_ranks", "[values] must not be null: [p]"),
        ),
        (
            "percentile_ranks",
            r#""field":"n","values":[]"#,
            build(
                "percentile_ranks",
                "[values] must not be an empty array: [p]",
            ),
        ),
        (
            "percentiles",
            r#""field":"n","tdigest":{"compression":-1}"#,
            vec![
                "[1:67] [percentiles] failed to parse field [tdigest]".to_owned(),
                "[1:67] [tdigest] failed to parse field [compression]".to_owned(),
                "[compression] must be greater than or equal to 0. Found [-1.0]".to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","tdigest":{"compression":"x"}"#,
            vec![
                "[1:67] [percentiles] failed to parse field [tdigest]".to_owned(),
                "[1:67] [tdigest] failed to parse field [compression]".to_owned(),
                "For input string: \"x\"".to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","tdigest":{"compression":true}"#,
            vec![
                "[1:67] [percentiles] failed to parse field [tdigest]".to_owned(),
                "[1:67] [tdigest] compression doesn't support values of type: VALUE_BOOLEAN"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","tdigest":{"compresion":1}"#,
            vec![
                "[1:66] [percentiles] failed to parse field [tdigest]".to_owned(),
                "[1:53] [tdigest] unknown field [compresion] did you mean [compression]?"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","tdigest":5"#,
            vec![
                "[1:52] [percentiles] tdigest doesn't support values of type: VALUE_NUMBER"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","percents":{}"#,
            vec![
                "[1:53] [percentiles] percents doesn't support values of type: START_OBJECT"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","percents":[1,"x"]"#,
            vec![
                "[1:56] [percentiles] failed to parse field [percents]".to_owned(),
                "For input string: \"x\"".to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","percents":"x""#,
            vec![
                "[1:53] [percentiles] failed to parse field [percents]".to_owned(),
                "For input string: \"x\"".to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","percents":[null]"#,
            vec![
                "[1:54] [percentiles] failed to parse field [percents]".to_owned(),
                "Current token (VALUE_NULL) not numeric, can not use numeric value accessors"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","keyed":"yes""#,
            vec![
                "[1:50] [percentiles] failed to parse field [keyed]".to_owned(),
                "Failed to parse value [yes] as only [true] or [false] are allowed.".to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","keyed":1"#,
            vec![
                "[1:50] [percentiles] keyed doesn't support values of type: VALUE_NUMBER"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":1"#,
            vec![
                "[1:38] [percentiles] field doesn't support values of type: VALUE_NUMBER"
                    .to_owned(),
            ],
        ),
        (
            "percentiles",
            r#""field":"n","method":"tdigest""#,
            vec!["[1:42] [percentiles] unknown field [method]".to_owned()],
        ),
        (
            "percentile_ranks",
            r#""field":"n","percents":[1]"#,
            vec!["[1:47] [percentile_ranks] unknown field [percents]".to_owned()],
        ),
        (
            "percentiles",
            r#""percents":[1]"#,
            vec!["Required one of fields [field, script], but none were specified. ".to_owned()],
        ),
        (
            "percentile_ranks",
            r#""field":"n","values":["NaN"]"#,
            vec!["Invalid value: NaN".to_owned()],
        ),
        (
            "percentile_ranks",
            r#""field":"n","values":[1,"-Infinity"]"#,
            vec!["Invalid value: -Infinity".to_owned()],
        ),
    ] {
        assert_eq!(refused(ty, inner), want, "{ty} {inner}");
    }
}

#[test]
fn options_the_node_does_not_serve_are_refused() {
    for (inner, what) in [
        (
            r#""field":"n","hdr":{}"#,
            "the [percentiles] aggregation's [hdr]",
        ),
        (
            r#""field":"n","format":"0.0""#,
            "the [percentiles] aggregation's [format]",
        ),
        (
            r#""field":"n","missing":1"#,
            "the [percentiles] aggregation's [missing]",
        ),
        (
            r#""field":"n","script":"x""#,
            "the [percentiles] aggregation's [script]",
        ),
        (
            r#""field":"n","value_type":"long""#,
            "the [percentiles] aggregation's [value_type]",
        ),
        (
            r#""field":"n","tdigest":{"compression":"NaN"}"#,
            "a [tdigest] compression of [NaN]",
        ),
        (
            r#""field":"n","tdigest":{"compression":"Infinity"}"#,
            "a [tdigest] compression of [Infinity]",
        ),
        (
            r#""field":"n","tdigest":{"compression":10000.5}"#,
            "a [tdigest] compression of [10000.5]",
        ),
    ] {
        assert_eq!(
            refused("percentiles", inner),
            [format!("{what} is not supported by this node yet")],
            "{inner}"
        );
    }
    // The largest compression served.
    assert_eq!(
        options("percentiles", r#","tdigest":{"compression":1e4}"#).compression,
        MAX_COMPRESSION
    );
}

fn digest(values: impl Iterator<Item = f64>, compression: f64) -> Digest {
    let mut d = MergingDigest::new(compression);
    for v in values {
        d.add(v).expect("not NaN");
    }
    let mut d = Digest::from_engine(d);
    d.settle();
    d
}

fn written(format: Format, value: Value) -> String {
    let mut out = Vec::new();
    write_aggregations(
        &mut out,
        &[Partial {
            name: "p".to_owned(),
            format,
            value,
        }],
    )
    .expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn value(ranks: bool, keys: &[f64], keyed: bool, d: Option<Digest>) -> Value {
    Value::Percentiles {
        ranks,
        keys: keys.to_vec(),
        keyed,
        compression: 100.0,
        digest: d,
    }
}

#[test]
fn percentiles_are_written_as_opensearch_writes_them() {
    let d = || Some(digest((0..200).map(f64::from), 100.0));
    assert_eq!(
        written(Format::Raw, value(false, &[1.0, 50.0, 50.0], true, d())),
        r#","aggregations":{"p":{"values":{"1.0":2.0,"50.0":99.5,"50.0":99.5}}}"#
    );
    assert_eq!(
        written(Format::Raw, value(false, &[25.0, 1e12 / 1e10], false, d())),
        r#","aggregations":{"p":{"values":[{"key":25.0,"value":49.5},{"key":100.0,"value":199.0}]}}"#
    );
    assert_eq!(
        written(Format::Raw, value(true, &[-1.0, 50.5, 1e9], true, d())),
        r#","aggregations":{"p":{"values":{"-1.0":0.0,"50.5":25.5,"1.0E9":100.0}}}"#
    );
    // No value: null, and no text for a date.
    assert_eq!(
        written(Format::Date, value(false, &[5.0], true, None)),
        r#","aggregations":{"p":{"values":{"5.0":null}}}"#
    );
    assert_eq!(
        written(
            Format::Date,
            value(true, &[5.0], false, Some(digest(std::iter::empty(), 100.0)))
        ),
        r#","aggregations":{"p":{"values":[{"key":5.0,"value":null}]}}"#
    );
    // A NaN percent: its key and value as strings.
    assert_eq!(
        written(Format::Raw, value(false, &[f64::NAN], false, d())),
        r#","aggregations":{"p":{"values":[{"key":"NaN","value":"NaN"}]}}"#
    );
    assert_eq!(
        written(Format::Raw, value(false, &[f64::NAN], true, d())),
        r#","aggregations":{"p":{"values":{"NaN":"NaN"}}}"#
    );
    // A date: each value also as text, a rank's too.
    let dates = || {
        Some(digest(
            [946_598_400_000.0, 1_262_304_000_000.0].into_iter(),
            100.0,
        ))
    };
    assert_eq!(
        written(Format::Date, value(false, &[0.0], true, dates())),
        r#","aggregations":{"p":{"values":{"0.0":9.465984E11,"0.0_as_string":"1999-12-31T00:00:00.000Z"}}}"#
    );
    assert_eq!(
        written(Format::Date, value(true, &[1e12], false, dates())),
        r#","aggregations":{"p":{"values":[{"key":1.0E12,"value":50.0,"value_as_string":"1970-01-01T00:00:00.050Z"}]}}"#
    );
}

#[test]
fn percentiles_reduce_as_opensearch_reduces_them() {
    let part = |range: std::ops::Range<i32>| {
        value(
            false,
            &[50.0],
            true,
            Some(digest(range.map(f64::from), 100.0)),
        )
    };
    let empty = value(false, &[50.0], true, None);
    let reduced = reduce(&[part(0..100), empty.clone(), part(100..200)]).expect("reduces");
    let mut want = Digest::new(100.0);
    want.add_digest(&digest((0..100).map(f64::from), 100.0));
    want.add_digest(&digest((100..200).map(f64::from), 100.0));
    want.settle();
    assert_eq!(reduced, value(false, &[50.0], true, Some(want)));
    let Value::Percentiles {
        digest: Some(d), ..
    } = &reduced
    else {
        panic!("{reduced:?}");
    };
    assert_eq!(d.size(), 200);
    assert_eq!(reduce(&[empty.clone(), empty.clone()]), Some(empty.clone()));
    // The first's keys, keyed and compression win.
    let other = Value::Percentiles {
        ranks: false,
        keys: vec![1.0],
        keyed: false,
        compression: 5.0,
        digest: None,
    };
    assert_eq!(reduce(&[empty.clone(), other.clone()]), Some(empty.clone()));
    // Ranks and percentiles, or another metric, do not reduce together.
    assert_eq!(
        reduce(&[empty.clone(), value(true, &[50.0], true, None)]),
        None
    );
    assert_eq!(reduce(&[empty.clone(), Value::Min(1.0)]), None);
    assert_eq!(reduce(&[Value::Min(1.0), empty]), None);
}

#[test]
fn a_digest_reserves_every_array_it_holds() {
    for compression in [0.0, 5.0, 10.0, 29.0, 30.0, 100.0, 1000.0, 12_345.6] {
        let engine = MergingDigest::new(compression);
        let held = (engine.weight.len() + engine.mean.len()) * 8
            + (engine.temp_weight.len() + engine.temp_mean.len()) * 8
            + engine.temp_weight.len() * (4 + 3 * 16 + 8);
        assert_eq!(
            tdigest::digest_bytes(compression),
            held + std::mem::size_of::<Digest>(),
            "{compression}"
        );
    }
    assert!(tdigest::digest_bytes(MAX_COMPRESSION) < 8 << 20);
}
