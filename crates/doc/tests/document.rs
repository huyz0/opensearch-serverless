//! What a document gives each field: the values the fixtures cannot show
//! (they record only acceptance and the mapping), `_ignored`, multi-fields,
//! date milliseconds, and the dynamic update.

use oss_doc::document::{self, FieldValue, ParsedDocument};
use oss_doc::mapping::{Mapping, MappingError, Num};

fn mapping(json: &str) -> Mapping {
    Mapping::parse(json.as_bytes()).unwrap()
}

fn parse(m: &Mapping, doc: &str) -> Result<ParsedDocument, MappingError> {
    document::parse(m, "1", doc.as_bytes())
}

fn values(m: &Mapping, doc: &str) -> Vec<(String, FieldValue)> {
    parse(m, doc).unwrap().fields
}

fn kv(name: &str, v: FieldValue) -> (String, FieldValue) {
    (name.to_owned(), v)
}

const TYPES: &str = r#"{"properties":{
  "t":{"type":"text","fields":{"raw":{"type":"keyword","ignore_above":3}}},
  "k":{"type":"keyword","null_value":"NV"},
  "l":{"type":"long"}, "i":{"type":"integer"}, "s":{"type":"short"}, "b":{"type":"byte"},
  "f":{"type":"float"}, "d":{"type":"double","null_value":2.5},
  "bo":{"type":"boolean","null_value":true},
  "dt":{"type":"date"}, "de":{"type":"date","format":"epoch_second","null_value":"10"},
  "im":{"type":"integer","ignore_malformed":true},
  "o":{"properties":{"x":{"type":"long"}}}}}"#;

#[test]
fn each_type_reads_its_values() {
    let m = mapping(TYPES);
    let got = values(
        &m,
        r#"{"t":"abcd","k":null,"l":"12.9","i":3.7,"s":"-2","b":[1,"2"],"f":"1.5","d":null,"bo":"","dt":"1970-01-01T00:00:01.5Z","de":"2.25","o":{"x":7},"o.x":8,"im":"bad"}"#,
    );
    assert_eq!(
        got,
        vec![
            kv("t", FieldValue::Text("abcd".into())),
            kv("k", FieldValue::Keyword("NV".into())),
            kv("l", FieldValue::Number(Num::Long(12))),
            kv("i", FieldValue::Number(Num::Int(3))),
            kv("s", FieldValue::Number(Num::Int(-2))),
            kv("b", FieldValue::Number(Num::Int(1))),
            kv("b", FieldValue::Number(Num::Int(2))),
            kv("f", FieldValue::Number(Num::Float(1.5))),
            kv("d", FieldValue::Number(Num::Double(2.5))),
            kv("bo", FieldValue::Bool(false)),
            kv("dt", FieldValue::Date(1_500)),
            kv("de", FieldValue::Date(2_250)),
            kv("o.x", FieldValue::Number(Num::Long(7))),
            kv("o.x", FieldValue::Number(Num::Long(8))),
        ]
    );
    let doc = parse(&m, r#"{"t":"abcd","im":"bad"}"#).unwrap();
    // `t.raw` is over its `ignore_above` and dropped, not listed, as the
    // Java drops it (fixtures/segments/types.json, `short_tag`);
    // `im` is malformed and ignored.
    assert_eq!(doc.ignored, vec!["im".to_owned()]);
    assert_eq!(doc.update, None);
    assert_eq!(doc.grown, None);
    let nulls = values(&m, r#"{"bo":null,"de":null,"t":null,"l":null,"o":null}"#);
    assert_eq!(
        nulls,
        vec![
            kv("bo", FieldValue::Bool(true)),
            kv("de", FieldValue::Date(10_000))
        ]
    );
    let multi = values(&m, r#"{"t":"ab"}"#);
    assert_eq!(
        multi,
        vec![
            kv("t", FieldValue::Text("ab".into())),
            kv("t.raw", FieldValue::Keyword("ab".into()))
        ]
    );
}

#[test]
fn dynamic_fields_and_their_update() {
    let m = mapping(r#"{"properties":{"o":{"properties":{"x":{"type":"long"}}}}}"#);
    let doc = parse(
        &m,
        r#"{"s":"x y","n":1,"o":{"y":true},"a.b":1.5,"when":"2024-01-02"}"#,
    )
    .unwrap();
    assert_eq!(
        doc.fields,
        vec![
            kv("s", FieldValue::Text("x y".into())),
            kv("s.keyword", FieldValue::Keyword("x y".into())),
            kv("n", FieldValue::Number(Num::Long(1))),
            kv("o.y", FieldValue::Bool(true)),
            kv("a.b", FieldValue::Number(Num::Float(1.5))),
            kv("when", FieldValue::Date(1_704_153_600_000)),
        ]
    );
    assert_eq!(
        doc.update.unwrap().to_json(),
        r#"{"properties":{"a":{"properties":{"b":{"type":"float"}}},"n":{"type":"long"},"o":{"properties":{"y":{"type":"boolean"}}},"s":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":256}}},"when":{"type":"date"}}}"#
    );
    let grown = doc.grown.unwrap();
    assert!(
        grown
            .to_json()
            .contains(r#""o":{"properties":{"x":{"type":"long"},"y":{"type":"boolean"}}}"#)
    );
    // The grown mapping indexes the same document with no further update.
    assert_eq!(
        parse(&grown, r#"{"s":"z","o":{"y":false}}"#)
            .unwrap()
            .update,
        None
    );
}

#[test]
fn dynamic_modes() {
    let strict = mapping(
        r#"{"dynamic":"strict_allow_templates","properties":{"f":{"type":"object","dynamic":"false_allow_templates"},"n":{"type":"object","dynamic":"true"}}}"#,
    );
    let e = parse(&strict, r#"{"x":1}"#).unwrap_err();
    assert_eq!(
        e.reason,
        "mapping set to strict_allow_templates, dynamic introduction of [x] within [_doc] is not allowed"
    );
    assert_eq!(
        parse(&strict, r#"{"x":null}"#).unwrap_err().ty,
        "strict_dynamic_mapping_exception"
    );
    assert_eq!(
        parse(&strict, r#"{"x":{}}"#).unwrap_err().ty,
        "strict_dynamic_mapping_exception"
    );
    assert_eq!(
        parse(&strict, r#"{"x":[]}"#).unwrap_err().ty,
        "strict_dynamic_mapping_exception"
    );
    assert_eq!(
        parse(&strict, r#"{"x.y":1}"#).unwrap_err().reason,
        "mapping set to strict_allow_templates, dynamic introduction of [x] within [_doc] is not allowed"
    );
    let ok = parse(
        &strict,
        r#"{"f":{"a":1,"b":{"c":1},"d":[1]},"n":{"a":"t"}}"#,
    )
    .unwrap();
    assert_eq!(
        ok.update.unwrap().to_json(),
        r#"{"properties":{"n":{"properties":{"a":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":256}}}}}}}"#
    );
    let off = mapping(r#"{"dynamic":false}"#);
    let doc = parse(&off, r#"{"a":1,"b":{"c":2},"d":[1,{"e":1}],"g.h":1}"#).unwrap();
    assert!(doc.fields.is_empty() && doc.update.is_none());
}

#[test]
fn refusals_outside_the_fixtures() {
    // Recorded refusals are in `fixtures/compat/mapping/`; these are the
    // ones a recording cannot make: malformed bodies, and the limits.
    let m = mapping(TYPES);
    let e = parse(&m, r#"{"l":1} {"#).unwrap_err();
    assert_eq!(e.reason, "failed to parse");
    let e = parse(&m, r#"{"l":"#).unwrap_err();
    assert_eq!(e.caused_by.unwrap().ty, "json_parse_exception");
    // A trailing dot is dropped as `String.split` drops it.
    assert_eq!(
        values(&m, r#"{"l.":4}"#),
        vec![kv("l", FieldValue::Number(Num::Long(4)))]
    );
    assert!(values(&m, r#"{"_doc_count":3,"l":1}"#).len() == 1);
}

#[test]
fn dotted_names_reach_existing_objects() {
    let m = mapping(
        r#"{"properties":{"o":{"properties":{"x":{"type":"long"}}},"sx":{"type":"object","dynamic":"strict","properties":{"in":{"type":"object","properties":{}}}},"off":{"type":"object","enabled":false}}}"#,
    );
    // A new field under an existing object, through a dotted name.
    let doc = parse(&m, r#"{"o.y":"t","off.z":{"a":1}}"#).unwrap();
    assert_eq!(
        doc.update.unwrap().to_json(),
        r#"{"properties":{"o":{"properties":{"y":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":256}}}}}}}"#
    );
    // Strict objects refuse, inherited by their children, through either form.
    let strict = |d: &str| parse(&m, d).unwrap_err().reason;
    assert_eq!(
        strict(r#"{"sx.q":1}"#),
        "mapping set to strict, dynamic introduction of [q] within [sx] is not allowed"
    );
    assert_eq!(
        strict(r#"{"sx":{"q":{}}}"#),
        "mapping set to strict, dynamic introduction of [q] within [sx] is not allowed"
    );
    assert_eq!(
        strict(r#"{"sx.in.q":[1]}"#),
        "mapping set to strict, dynamic introduction of [q] within [sx.in] is not allowed"
    );
    assert_eq!(
        strict(r#"{"sx.in.q.r":1}"#),
        "mapping set to strict, dynamic introduction of [q] within [sx.in] is not allowed"
    );
    // A disabled root indexes nothing.
    let off = mapping(r#"{"enabled":false}"#);
    assert!(parse(&off, r#"{"a":{"b":[1]}}"#).unwrap().fields.is_empty());
}

#[test]
fn the_total_fields_limit_holds_while_fields_are_added() {
    let m = Mapping::default();
    let fields = |n: usize| {
        let body: Vec<String> = (0..n).map(|i| format!("\"f{i}\":{i}")).collect();
        format!("{{{}}}", body.join(","))
    };
    let at = parse(&m, &fields(1000)).unwrap();
    assert_eq!(at.grown.unwrap().field_count(), 1000);
    let e = parse(&m, &fields(1001)).unwrap_err();
    assert_eq!(e.reason, "Limit of total fields [1000] has been exceeded");
    // Text fields count their keyword multi-field too.
    let e = parse(
        &m,
        &format!(
            "{{{}}}",
            (0..501)
                .map(|i| format!("\"t{i}\":\"x\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    )
    .unwrap_err();
    assert_eq!(e.reason, "Limit of total fields [1000] has been exceeded");
}

#[test]
fn numeric_detection_reads_strings_as_java_parses_them() {
    let m = mapping(r#"{"numeric_detection":true}"#);
    // Past `Long.MAX_VALUE`, `Long.parseLong` fails and `Double.parseDouble`
    // reads it, so the field is a float.
    let doc = parse(&m, r#"{"n":"-12","big":"99999999999999999999"}"#).unwrap();
    assert_eq!(
        doc.update.unwrap().to_json(),
        r#"{"properties":{"big":{"type":"float"},"n":{"type":"long"}}}"#
    );
}

#[test]
fn duplicate_keys_are_found_per_object() {
    let m = mapping("{}");
    // An array closes without closing the object around it.
    let e = parse(&m, r#"{"a":[1],"a":2}"#).unwrap_err();
    let cause = e.caused_by.map_or(e.reason, |c| c.reason);
    assert!(
        cause.starts_with(r#"Duplicate Object property "a""#),
        "{cause}"
    );
    // A key repeated in another object is no duplicate.
    assert!(parse(&m, r#"{"o":{"x":1},"x":2,"p":[{"y":1},{"y":2}]}"#).is_ok());
}

#[test]
fn a_keyword_at_its_ignore_above_is_kept() {
    let m = mapping(TYPES);
    assert_eq!(
        values(&m, r#"{"t":"abc"}"#),
        vec![
            kv("t", FieldValue::Text("abc".into())),
            kv("t.raw", FieldValue::Keyword("abc".into())),
        ]
    );
}

#[test]
fn coerced_strings_at_the_range_edges() {
    let m = mapping(TYPES);
    let cause = |doc: &str| {
        let e = parse(&m, doc).unwrap_err();
        e.caused_by.map_or(e.reason, |c| c.reason)
    };
    // `IntegerFieldMapper` compares the double it read with the range,
    // which NaN passes, and `(int) NaN` is 0.
    assert_eq!(
        values(&m, r#"{"i":"NaN","s":"NaN","b":"NaN","l":0}"#)
            .into_iter()
            .take(3)
            .collect::<Vec<_>>(),
        vec![
            kv("i", FieldValue::Number(Num::Int(0))),
            kv("s", FieldValue::Number(Num::Int(0))),
            kv("b", FieldValue::Number(Num::Int(0))),
        ]
    );
    assert_eq!(
        values(&m, r#"{"i":"-2147483648"}"#),
        vec![kv("i", FieldValue::Number(Num::Int(i32::MIN)))]
    );
    assert_eq!(
        cause(r#"{"i":"-2147483649"}"#),
        "Value [-2147483649] is out of range for an integer"
    );
    assert_eq!(
        cause(r#"{"s":"-32768.5"}"#),
        "Value [-32768.5] is out of range for a short"
    );
    assert_eq!(
        cause(r#"{"d":"NaN"}"#),
        "[double] supports only finite values, but got [NaN]"
    );
}

#[test]
fn a_long_string_parses_exactly_or_through_big_decimal() {
    let m = mapping(TYPES);
    let cause = |doc: &str| {
        let e = parse(&m, doc).unwrap_err();
        e.caused_by.map_or(e.reason, |c| c.reason)
    };
    // `Long.parseLong` first, so no double rounds it.
    assert_eq!(
        values(&m, r#"{"l":"9007199254740993"}"#),
        vec![kv(
            "l",
            FieldValue::Number(Num::Long(9_007_199_254_740_993))
        )]
    );
    for text in ["inf", "+-5", "x.5", "1.x"] {
        assert_eq!(
            cause(&format!(r#"{{"l":"{text}"}}"#)),
            format!("For input string: \"{text}\""),
        );
    }
}

#[test]
fn objects_reach_the_depth_limit() {
    let m = mapping("{}");
    let deep = |n: usize| (0..n).fold("1".to_string(), |inner, _| format!(r#"{{"a":{inner}}}"#));
    // Stock OpenSearch takes twenty objects, the root among them, and
    // refuses a twenty-first.
    assert_eq!(parse(&m, &deep(20)).unwrap().fields.len(), 1);
    let e = parse(&m, &deep(21)).unwrap_err();
    assert!(
        e.caused_by
            .is_some_and(|c| c.reason.starts_with("The depth of the field has exceeded")),
    );
    // Unmapped objects are skipped, not counted.
    let skipped = mapping(r#"{"dynamic":false}"#);
    assert!(parse(&skipped, &deep(23)).is_ok());
}

#[test]
fn a_metadata_name_inside_an_object_is_a_field() {
    let doc = parse(&mapping("{}"), r#"{"o":{"_id":"x"}}"#).unwrap();
    assert_eq!(doc.fields.first().map(|(n, _)| n.as_str()), Some("o._id"));
}

#[test]
fn arrays_reach_the_depth_limit() {
    let m = mapping("{}");
    let nested = |n: usize| format!(r#"{{"z":{}1{}}}"#, "[".repeat(n), "]".repeat(n));
    // Stock OpenSearch takes twenty arrays inside each other.
    assert_eq!(parse(&m, &nested(20)).unwrap().fields.len(), 1);
    assert!(parse(&m, &nested(21)).is_err());
}

#[test]
fn a_dotted_name_reaches_a_mapped_leaf() {
    let m = mapping(r#"{"properties":{"o":{"properties":{"x":{"type":"keyword"}}}}}"#);
    let doc = parse(&m, r#"{"o.x":5}"#).unwrap();
    assert_eq!(doc.fields, vec![kv("o.x", FieldValue::Keyword("5".into()))]);
    assert_eq!(doc.update, None);
}

#[test]
fn leap_days_follow_the_gregorian_rules() {
    let m = mapping(TYPES);
    for (text, ok) in [
        ("2000-02-29", true),
        ("2024-02-29", true),
        ("1900-02-29", false),
        ("2023-02-29", false),
        ("2020-13-01", false),
        ("2020-00-01", false),
    ] {
        let doc = parse(&m, &format!(r#"{{"dt":"{text}"}}"#));
        assert_eq!(doc.is_ok(), ok, "{text}: {doc:?}");
    }
    assert_eq!(
        values(&m, r#"{"dt":"2000-02-29"}"#),
        vec![kv("dt", FieldValue::Date(951_782_400_000))]
    );
}

#[test]
fn date_edges_as_java_time_reads_them() {
    let m = mapping(
        r#"{"properties":{"dt":{"type":"date"},"em":{"type":"date","format":"epoch_millis"},
        "es":{"type":"date","format":"epoch_second"},"sl":{"type":"date","format":"yyyy/MM/dd HH:mm:ss"},
        "do":{"type":"date","format":"date_optional_time"},"dc":{"type":"date","format":"dateOptionalTime"}}}"#,
    );
    for (doc, ok) in [
        (r#"{"dt":"2020-01-01T00:00:00+18:00"}"#, true),
        (r#"{"dt":"2020-01-01T00:00:00+18:30"}"#, false),
        (r#"{"dt":"2020-01-01T00:00:00+19:00"}"#, false),
        (r#"{"dt":"2020-01-01T00:00:00+05:59"}"#, true),
        (r#"{"dt":"2020-01-01T00:00:00+05:60"}"#, false),
        (r#"{"em":"1.123456"}"#, true),
        (r#"{"em":"1.1234567"}"#, false),
        (r#"{"es":"1.123456789"}"#, true),
        (r#"{"es":"1.1234567891"}"#, false),
        (r#"{"sl":"2020/01/01 23:59:59"}"#, true),
        (r#"{"sl":"2020/01/01 24:00:00"}"#, false),
        (r#"{"do":"2020-01-01","dc":"2020-01-01"}"#, true),
    ] {
        assert_eq!(parse(&m, doc).is_ok(), ok, "{doc}");
    }
}

#[test]
fn more_edges_as_stock_opensearch_answered() {
    let m = mapping(
        r#"{"properties":{"dt":{"type":"date"},"sl":{"type":"date","format":"yyyy/MM/dd HH:mm:ss"},"n":{"type":"integer"}}}"#,
    );
    // A negative offset is behind UTC.
    assert_eq!(
        values(&m, r#"{"dt":"2020-01-01T00:00:00-05:00"}"#),
        vec![kv("dt", FieldValue::Date(1_577_854_800_000))]
    );
    for doc in [
        r#"{"dt":"2020-01-01T00:60:00Z"}"#,
        r#"{"dt":"2020-01-01T00:00:60Z"}"#,
        r#"{"n":"2147483648"}"#,
    ] {
        assert!(parse(&m, doc).is_err(), "{doc}");
    }
    assert!(parse(&m, r#"{"n":"2147483647"}"#).is_ok());
    // A day the month lacks, at the last second of the day.
    let e = parse(&m, r#"{"sl":"2023/02/30 23:59:59"}"#).unwrap_err();
    let cause = e.caused_by.expect("a cause");
    assert_eq!(
        (cause.ty, cause.reason.as_str()),
        ("date_time_exception", "Invalid date 'FEBRUARY 30'")
    );
}
