//! Mapping edge cases the recorded fixtures do not reach: the `meta`
//! limits, number ranges in `null_value`, integer parameters given as
//! decimals, JSON escaping, dotted names that share a parent, and the
//! builders dynamic mapping uses.

use oss_writer::mapping::json::{Json, java_double, java_float, parse as parse_json};
use oss_writer::mapping::{Dynamic, FieldType, Leaf, Mapper, Mapping, MappingError, Value};

fn parse(json: &str) -> Result<Mapping, MappingError> {
    Mapping::parse(json.as_bytes())
}

fn cause(json: &str) -> String {
    let e = parse(json).unwrap_err();
    e.caused_by.map_or(e.reason, |c| c.reason)
}

fn meta(members: &str) -> String {
    format!(r#"{{"properties":{{"a":{{"type":"keyword","meta":{{{members}}}}}}}}}"#)
}

#[test]
fn meta_limits_are_javas() {
    let hundred: Vec<String> = (0..100).map(|i| format!(r#""k{i}":"v""#)).collect();
    assert!(parse(&meta(&hundred.join(","))).is_ok());
    let over: Vec<String> = (0..101).map(|i| format!(r#""k{i}":"v""#)).collect();
    assert_eq!(
        cause(&meta(&over.join(","))),
        "[meta] can't have more than 100 entries, but got 101 on field [a]"
    );
    let longest_key = "k".repeat(255);
    assert!(parse(&meta(&format!(r#""{longest_key}":"v""#))).is_ok());
    let longest_value = "v".repeat(10_000);
    assert!(parse(&meta(&format!(r#""k":"{longest_value}""#))).is_ok());
    let key = "k".repeat(256);
    assert_eq!(
        cause(&meta(&format!(r#""{key}":"v""#))),
        format!("[meta] keys can't be longer than 255 chars, but got [{key}] for field [a]")
    );
    let value = "v".repeat(10_001);
    assert_eq!(
        cause(&meta(&format!(r#""k":"{value}""#))),
        format!("[meta] values can't be longer than 10000 chars, but got [{value}] for field [a]")
    );
    // Keys sort as Java's TreeMap sorts them: by UTF-16 unit.
    let m = parse(&meta(r#""｡":"x","😀":"y""#)).unwrap();
    assert!(
        m.to_json().contains(r#""meta":{"😀":"y","｡":"x"}"#),
        "{}",
        m.to_json()
    );
}

#[test]
fn meta_values_and_conflicts_render_as_java_does() {
    assert_eq!(
        cause(&meta(r#""k":[1,2]"#)),
        "[meta] values can only be strings, but got ArrayList[[1, 2]] for field [a]"
    );
    assert_eq!(
        cause(&meta(r#""k":{"x":true}"#)),
        "[meta] values can only be strings, but got LinkedHashMap[{x=true}] for field [a]"
    );
    assert_eq!(
        cause(&meta(r#""k":99999999999999999999"#)),
        "[meta] values can only be strings, but got BigInteger[99999999999999999999] for field [a]"
    );
    let a = parse(&meta(r#""k":"1""#)).unwrap();
    let b = parse(&meta(r#""k":"2","j":"3""#)).unwrap();
    // `meta` is updateable, so a change merges.
    assert!(
        a.merge(&b)
            .unwrap()
            .to_json()
            .contains(r#""meta":{"j":"3","k":"2"}"#)
    );
}

#[test]
fn integer_parameters_read_as_java_reads_them() {
    let gap = |v: &str| {
        format!(r#"{{"properties":{{"a":{{"type":"text","position_increment_gap":{v}}}}}}}"#)
    };
    assert!(
        parse(&gap("5.0"))
            .unwrap()
            .to_json()
            .contains(r#""position_increment_gap":5"#)
    );
    assert_eq!(cause(&gap("5.5")), "5.5 is not an integer value");
    assert_eq!(cause(&gap("3e10")), "integer overflow");
    assert_eq!(
        cause(&gap("99999999999999999999")),
        "BigInteger out of long range"
    );
}

#[test]
fn number_null_values_keep_javas_range_rules() {
    let null = |ty: &str, v: &str, coerce: bool| {
        format!(r#"{{"properties":{{"a":{{"type":"{ty}","coerce":{coerce},"null_value":{v}}}}}}}"#)
    };
    assert_eq!(
        cause(&null("long", "1e19", true)),
        "Value [1.0E19] is out of range for a long"
    );
    assert_eq!(
        cause(&null("long", "1.5", false)),
        "Value [1.5] has a decimal part"
    );
    assert_eq!(
        cause(&null("long", "9223372036854775807.0", true)),
        "Value [9.223372036854776E18] is out of range for a long"
    );
    // `null_value` is parsed without coercion, whatever `coerce` says.
    assert_eq!(
        cause(&null("long", "12.7", true)),
        "Value [12.7] has a decimal part"
    );
    let ok = parse(&null("long", "12.0", true)).unwrap();
    assert!(
        ok.to_json().contains(r#""null_value":12"#),
        "{}",
        ok.to_json()
    );
    assert_eq!(
        cause(&null("float", "1e39", true)),
        "[float] supports only finite values, but got [Infinity]"
    );
    assert!(parse(&null("short", "12", true)).is_ok());
    let f = parse(&null("float", "1.5", true)).unwrap();
    let g = parse(&null("float", "2.5", true)).unwrap();
    assert!(
        f.merge(&g)
            .unwrap_err()
            .reason
            .contains("from [1.5] to [2.5]")
    );
}

#[test]
fn java_number_strings() {
    assert_eq!(java_float(f32::INFINITY), "Infinity");
    assert_eq!(java_float(f32::NEG_INFINITY), "-Infinity");
    assert_eq!(java_float(-0.0), "-0.0");
    assert_eq!(java_float(0.0), "0.0");
    assert_eq!(java_double(1e19), "1.0E19");
}

#[test]
fn json_values_escape_and_render_as_jackson_and_java() {
    let v =
        parse_json(br#"{"s":"q\"b\\n\nr\rt\tb\bf\f\u0001","a":[1,null,true],"o":{"x":1,"y":2}}"#)
            .unwrap();
    assert_eq!(
        v.to_json(),
        r#"{"s":"q\"b\\n\nr\rt\tb\bf\f\u0001","a":[1,null,true],"o":{"x":1,"y":2}}"#
    );
    assert_eq!(v.get("o").unwrap().java_string(), "{x=1, y=2}");
    assert_eq!(v.get("a").unwrap().java_string(), "[1, null, true]");
    assert!(v.get("missing").is_none());
    assert!(Json::Null.get("x").is_none());
    assert!(parse_json(br#"1 2"#).is_err());
    let big = vec![b' '; 8 * 1024 * 1024 + 1];
    assert_eq!(
        parse_json(&big).unwrap_err().reason,
        "the mapping is larger than its limit"
    );
}

#[test]
fn dotted_names_sharing_a_parent_merge() {
    let m = parse(r#"{"properties":{"o.x":{"type":"long"},"o.y":{"type":"keyword"},"o":{"properties":{"z":{"type":"boolean"}}},"e":[]}}"#).unwrap();
    assert_eq!(
        m.to_json(),
        r#"{"properties":{"o":{"properties":{"x":{"type":"long"},"y":{"type":"keyword"},"z":{"type":"boolean"}}}}}"#
    );
    let Some(Mapper::Object(o)) = m.root().child("o") else {
        panic!("o is an object")
    };
    assert_eq!(o.children().len(), 3);
    assert_eq!(
        cause(r#"{"properties":{"o.":{"properties":{}}}}"#),
        "name cannot be empty string"
    );
    assert!(parse(r#"{"properties":{"o":{"properties":[]}}}"#).is_ok());
}

#[test]
fn dynamic_names_round_trip() {
    for name in [
        "true",
        "false",
        "strict",
        "strict_allow_templates",
        "false_allow_templates",
    ] {
        let m = parse(&format!(r#"{{"dynamic":"{name}"}}"#)).unwrap();
        assert_eq!(m.to_json(), format!(r#"{{"dynamic":"{name}"}}"#));
        assert_eq!(
            Dynamic::parse(&Json::Str(name.into())).unwrap().name(),
            name
        );
    }
}

#[test]
fn builders_for_dynamic_fields() {
    let mut m = Mapping::default();
    let mut text = Leaf::new(FieldType::Text);
    let mut kw = Leaf::new(FieldType::Keyword);
    kw.set("ignore_above", Value::Int(256));
    kw.set("no_such_parameter", Value::Int(1));
    text.add_multi_field("keyword", kw);
    m.root_mut().put("t", Mapper::Leaf(text));
    assert_eq!(
        m.to_json(),
        r#"{"properties":{"t":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":256}}}}}"#
    );
    let e = MappingError::strict_dynamic("b", "_doc");
    assert_eq!(
        e.to_string(),
        "strict_dynamic_mapping_exception: mapping set to strict, dynamic introduction of [b] within [_doc] is not allowed"
    );
}

#[test]
fn norms_may_only_be_turned_off() {
    let field = |ty: &str, norms: &str| {
        parse(&format!(
            r#"{{"properties":{{"a":{{"type":"{ty}"{norms}}}}}}}"#
        ))
        .unwrap()
    };
    let merge = |from: &Mapping, to: &Mapping| from.merge(to).map(|_| ()).map_err(|e| e.reason);
    let on = field("text", "");
    let off = field("text", r#","norms":false"#);
    assert_eq!(merge(&on, &on), Ok(()));
    assert_eq!(merge(&off, &off), Ok(()));
    assert_eq!(merge(&on, &off), Ok(()));
    assert!(
        merge(&off, &on)
            .unwrap_err()
            .contains("Cannot update parameter [norms] from [false] to [true]")
    );
    let keyword = field("keyword", "");
    assert_eq!(merge(&keyword, &keyword), Ok(()));
}

#[test]
fn an_unset_explicit_flag_remembers_it_was_not_set() {
    let long = Leaf::new(FieldType::from_name("long").unwrap());
    assert_eq!(long.get("coerce"), Some(&Value::Explicit(true, false)));
    assert_eq!(
        long.get("ignore_malformed"),
        Some(&Value::Explicit(false, false))
    );
    let date = Leaf::new(FieldType::Date);
    assert_eq!(
        date.get("ignore_malformed"),
        Some(&Value::Explicit(false, false))
    );
}

#[test]
fn routing_is_optional_and_ignores_type() {
    assert!(!parse("{}").unwrap().routing_required());
    // `ParametrizedFieldMapper.parse` drops a `type` it has no parameter for.
    let m = parse(r#"{"_routing":{"type":"x","required":true}}"#).unwrap();
    assert!(m.routing_required());
    // Setting the default explicitly is no change, before or after.
    let off = parse("{}")
        .unwrap()
        .merge(&parse(r#"{"_routing":{"required":false}}"#).unwrap())
        .unwrap();
    assert!(!off.routing_required());
    assert!(m.merge(&m).unwrap().routing_required());
}

#[test]
fn depth_limit_names_the_first_object_past_it() {
    let chain = |n: usize| {
        (0..n).fold(r#"{"type":"keyword"}"#.to_string(), |inner, i| {
            format!(r#"{{"properties":{{"o{i}":{inner}}}}}"#)
        })
    };
    // Nineteen objects under the root, the last holding a field, stay
    // within the limit; with twenty, the twentieth is named.
    assert!(parse(&chain(20)).is_ok());
    let path = (1..21)
        .rev()
        .map(|i| format!("o{i}"))
        .collect::<Vec<_>>()
        .join(".");
    assert_eq!(
        parse(&chain(21)).unwrap_err().reason,
        format!("Limit of mapping depth [20] has been exceeded due to object field [{path}]")
    );
}

#[test]
fn multi_fields_must_be_an_object_or_empty() {
    assert!(parse(r#"{"properties":{"k":{"type":"keyword","fields":[]}}}"#).is_ok());
    let e = cause(r#"{"properties":{"k":{"type":"keyword","fields":["x"]}}}"#);
    assert!(
        e.starts_with("expected map for property [fields] on field [[x]]"),
        "{e}"
    );
}

#[test]
fn a_field_of_only_enabled_is_an_object() {
    let m = parse(r#"{"properties":{"o":{"enabled":false}}}"#).unwrap();
    assert!(
        m.to_json()
            .contains(r#""o":{"type":"object","enabled":false}"#),
        "{}",
        m.to_json()
    );
    assert_eq!(
        cause(r#"{"properties":{"o":{"enabled":false,"dynamic":true}}}"#),
        "No type specified for field [o]"
    );
}

#[test]
fn properties_must_be_an_object_or_empty() {
    let m = parse(r#"{"properties":{"o":{"properties":[]}}}"#).unwrap();
    assert!(
        m.to_json().contains(r#""o":{"type":"object"}"#),
        "{}",
        m.to_json()
    );
    assert_eq!(
        cause(r#"{"properties":{"o":{"properties":["x"]}}}"#),
        "properties must be a map type"
    );
}

#[test]
fn an_objects_include_in_all_is_dropped() {
    let m = parse(
        r#"{"properties":{"o":{"include_in_all":true,"properties":{"a":{"type":"keyword"}}}}}"#,
    )
    .unwrap();
    assert_eq!(
        m.to_json(),
        r#"{"properties":{"o":{"properties":{"a":{"type":"keyword"}}}}}"#
    );
}

#[test]
fn a_position_increment_gap_of_one_is_not_the_default() {
    // Recorded from stock OpenSearch: the default is -1, use the analyzer's.
    let json = r#"{"properties":{"u":{"type":"text","position_increment_gap":1}}}"#;
    assert_eq!(parse(json).unwrap().to_json(), json);
}
