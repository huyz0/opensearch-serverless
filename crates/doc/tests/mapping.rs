//! The writer's mapping beyond the recorded fixtures: what it refuses as
//! unsupported, the shell's hash-ordered stored mappings, limits and the
//! leaf listing.

use oss_doc::mapping::{FieldType, Mapping, NumType, Value};

fn parse(json: &str) -> Result<Mapping, oss_doc::mapping::MappingError> {
    Mapping::parse(json.as_bytes())
}

#[test]
fn unsupported_types_and_parameters_are_refused_as_unsupported() {
    for json in [
        r#"{"properties":{"a":{"type":"nested"}}}"#,
        r#"{"properties":{"a":{"type":"ip"}}}"#,
        r#"{"properties":{"a":{"type":"text","analyzer":"english"}}}"#,
        r#"{"properties":{"a":{"type":"text","term_vector":"yes"}}}"#,
        r#"{"properties":{"a":{"type":"text","index_phrases":true}}}"#,
        r#"{"properties":{"a":{"type":"keyword","normalizer":"lower"}}}"#,
        r#"{"properties":{"a":{"type":"keyword","copy_to":"b"}}}"#,
        r#"{"properties":{"a":{"type":"date","format":"basic_date"}}}"#,
        r#"{"properties":{"a":{"type":"date","locale":"fr"}}}"#,
        r#"{"properties":{"a":{"type":"long","skip_list":true}}}"#,
        r#"{"properties":{"a":{"type":"text","fields":{"b":{"type":"keyword","fields":{"c":{"type":"keyword"}}}}}}}"#,
        r#"{"properties":{"o":{"type":"object","disable_objects":true}}}"#,
        r#"{"dynamic_templates":[]}"#,
        r#"{"_source":{"enabled":false}}"#,
        r#"{"_meta":{"a":1}}"#,
    ] {
        let e = parse(json).unwrap_err();
        assert!(e.is_unsupported(), "{json}: {e:?}");
    }
}

#[test]
fn default_valued_unsupported_parameters_are_accepted() {
    let m = parse(r#"{"properties":{"a":{"type":"text","term_vector":"no","analyzer":"standard"},"b":{"type":"keyword","normalizer":"default"}}}"#).unwrap();
    assert_eq!(
        m.to_json(),
        r#"{"properties":{"a":{"type":"text","analyzer":"standard"},"b":{"type":"keyword"}}}"#
    );
}

#[test]
fn a_hash_ordered_mapping_reads_as_the_same_mapping() {
    // The shell re-serializes merged mappings through a HashMap.
    let stored = r#"{"properties":{"t":{"fields":{"keyword":{"ignore_above":256,"type":"keyword"}},"type":"text"},"n":{"type":"long"}},"dynamic":"strict"}"#;
    let canonical = r#"{"dynamic":"strict","properties":{"n":{"type":"long"},"t":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":256}}}}}"#;
    let a = Mapping::from_descriptor(Some(stored)).unwrap();
    let b = Mapping::from_descriptor(Some(canonical)).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.to_json(), canonical);
    assert_eq!(Mapping::from_descriptor(None).unwrap(), Mapping::default());
    assert_eq!(Mapping::default().to_json(), "{}");
}

#[test]
fn leaves_list_fields_and_multi_fields_by_full_name() {
    let m = parse(r#"{"properties":{"o":{"properties":{"x":{"type":"integer"}}},"t":{"type":"text","fields":{"raw":{"type":"keyword"}}},"a.b":{"type":"boolean"}}}"#).unwrap();
    let names: Vec<(String, FieldType)> =
        m.leaves().into_iter().map(|(n, l)| (n, l.ty())).collect();
    assert_eq!(
        names,
        vec![
            ("a.b".to_string(), FieldType::Boolean),
            ("o.x".to_string(), FieldType::Number(NumType::Integer)),
            ("t".to_string(), FieldType::Text),
            ("t.raw".to_string(), FieldType::Keyword),
        ]
    );
    let leaves = m.leaves();
    let raw = leaves.iter().find(|(n, _)| n == "t.raw").unwrap().1;
    assert_eq!(raw.get("ignore_above"), Some(&Value::Int(i32::MAX)));
}

#[test]
fn total_fields_and_depth_limits() {
    let mut props = Vec::new();
    for i in 0..1001 {
        props.push(format!(r#""f{i}":{{"type":"keyword"}}"#));
    }
    let e = parse(&format!(r#"{{"properties":{{{}}}}}"#, props.join(","))).unwrap_err();
    assert_eq!(e.reason, "Limit of total fields [1000] has been exceeded");
    let deep = (0..21).fold(r#"{"type":"keyword"}"#.to_string(), |inner, i| {
        format!(r#"{{"properties":{{"o{i}":{inner}}}}}"#)
    });
    let e = parse(&deep).unwrap_err();
    assert!(
        e.reason
            .starts_with("Limit of mapping depth [20] has been exceeded"),
        "{e:?}"
    );
    // A merge past the limit is refused the same way.
    let base = parse(&format!(
        r#"{{"properties":{{{}}}}}"#,
        props[..1000].join(",")
    ))
    .unwrap();
    let more = parse(r#"{"properties":{"extra":{"type":"long"}}}"#).unwrap();
    assert_eq!(
        base.merge(&more).unwrap_err().reason,
        "Limit of total fields [1000] has been exceeded"
    );
}

#[test]
fn malformed_json_and_duplicates_are_refused() {
    assert!(parse("{").is_err());
    assert!(parse("[]").is_err());
    let e = parse(r#"{"properties":{"a":{"type":"long"},"a":{"type":"long"}}}"#).unwrap_err();
    assert!(e.reason.contains("Duplicate field 'a'"), "{e:?}");
    // A typed wrapper, as an older stored mapping may carry, is unwrapped.
    let m = parse(r#"{"_doc":{"properties":{"a":{"type":"long"}}}}"#).unwrap();
    assert_eq!(m.to_json(), r#"{"properties":{"a":{"type":"long"}}}"#);
}

#[test]
fn routing_and_detection_merge() {
    let m = parse(r#"{"_routing":{"required":true},"date_detection":false}"#).unwrap();
    assert!(m.routing_required() && !m.date_detection() && !m.numeric_detection());
    let e = m
        .merge(&parse(r#"{"_routing":{"required":false}}"#).unwrap())
        .unwrap_err();
    assert!(
        e.reason
            .contains("Cannot update parameter [required] from [true] to [false]"),
        "{e:?}"
    );
    let grown = m
        .merge(&parse(r#"{"numeric_detection":true}"#).unwrap())
        .unwrap();
    assert_eq!(
        grown.to_json(),
        r#"{"_routing":{"required":true},"date_detection":false,"numeric_detection":true}"#
    );
}
