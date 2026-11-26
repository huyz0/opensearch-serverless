//! docs/specs/nested-fields AC-1 and AC-2: nested objects in a mapping.

use oss_doc::mapping::{Mapping, ParseOptions};

const ON: ParseOptions = ParseOptions {
    knn: None,
    nested: true,
};

const NESTED: &str = r#"{"properties":{"title":{"type":"keyword"},"c":{"type":"nested","properties":{"w":{"type":"keyword"},"d":{"type":"nested","properties":{"x":{"type":"long"}}}}},"o":{"properties":{"n":{"type":"nested","properties":{"y":{"type":"long"}}}}}}}"#;

fn parse(text: &str) -> Mapping {
    Mapping::parse_with(text.as_bytes(), ON).expect("parses")
}

#[test]
fn nested_objects_parse_alone_inside_nested_and_inside_plain_objects() {
    let m = parse(NESTED);
    // The leaves keep their full names; the nested objects are no leaf.
    let names: Vec<String> = m.leaves().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["c.d.x", "c.w", "o.n.y", "title"]);
    assert!(m.is_nested("c") && m.is_nested("c.d") && m.is_nested("o.n"));
    assert!(!m.is_nested("o") && !m.is_nested("c.w") && !m.is_nested("title"));
    assert!(m.has_nested());
    assert!(!parse(r#"{"properties":{"a":{"type":"keyword"}}}"#).has_nested());
}

#[test]
fn a_nested_object_is_written_as_opensearch_writes_it() {
    let m = parse(NESTED);
    assert_eq!(
        m.to_json(),
        r#"{"properties":{"c":{"type":"nested","properties":{"d":{"type":"nested","properties":{"x":{"type":"long"}}},"w":{"type":"keyword"}}},"o":{"properties":{"n":{"type":"nested","properties":{"y":{"type":"long"}}}}},"title":{"type":"keyword"}}}"#
    );
    assert_eq!(parse(&m.to_json()).to_json(), m.to_json());
    // One with no properties and a dynamic setting.
    let bare = parse(r#"{"properties":{"c":{"type":"nested","dynamic":"strict"}}}"#);
    assert_eq!(
        bare.to_json(),
        r#"{"properties":{"c":{"type":"nested","dynamic":"strict"}}}"#
    );
}

#[test]
fn a_caller_that_does_not_index_nested_is_told_it_is_unsupported() {
    let e = Mapping::parse(NESTED.as_bytes()).expect_err("refused");
    assert!(e.is_unsupported(), "{e:?}");
    let off = ParseOptions {
        knn: None,
        nested: false,
    };
    assert!(Mapping::parse_with(NESTED.as_bytes(), off).is_err());
    // A stored mapping is read whatever the setting is now.
    assert!(Mapping::from_descriptor(Some(NESTED)).is_ok());
    assert!(Mapping::from_descriptor_pinned(Some(NESTED)).is_ok());
}

#[test]
fn what_is_not_indexed_in_a_nested_object_is_unsupported() {
    for body in [
        r#""include_in_parent":true"#,
        r#""include_in_root":true"#,
        r#""properties":{"v":{"type":"knn_vector","dimension":3,"method":{"name":"hnsw","engine":"lucene"}}}"#,
    ] {
        let text = format!(r#"{{"properties":{{"c":{{"type":"nested",{body}}}}}}}"#);
        let both = ParseOptions {
            knn: Some(true),
            nested: true,
        };
        let e = Mapping::parse_with(text.as_bytes(), both).expect_err("refused");
        assert!(e.is_unsupported(), "{body}: {e:?}");
    }
    // `false` is the default, and is accepted.
    parse(
        r#"{"properties":{"c":{"type":"nested","include_in_parent":false,"include_in_root":false}}}"#,
    );
}

#[test]
fn merging_a_nested_object_with_a_plain_one_fails_either_way() {
    let nested =
        parse(r#"{"properties":{"c":{"type":"nested","properties":{"w":{"type":"keyword"}}}}}"#);
    let plain = parse(r#"{"properties":{"c":{"properties":{"w":{"type":"keyword"}}}}}"#);
    assert_eq!(
        nested.merge(&plain).expect_err("fails").reason,
        "cannot change object mapping from nested to non-nested"
    );
    assert_eq!(
        plain.merge(&nested).expect_err("fails").reason,
        "cannot change object mapping from non-nested to nested"
    );
    // The same nested object merges, and takes a new field.
    let more =
        parse(r#"{"properties":{"c":{"type":"nested","properties":{"z":{"type":"long"}}}}}"#);
    let merged = nested.merge(&more).expect("merges");
    assert!(merged.is_nested("c"));
    let names: Vec<String> = merged.leaves().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["c.w", "c.z"]);
}

#[test]
fn more_than_a_hundred_nested_objects_are_refused() {
    let many = |n: usize| {
        let fields: Vec<String> = (0..n)
            .map(|i| format!(r#""n{i}":{{"type":"nested"}}"#))
            .collect();
        format!(r#"{{"properties":{{{}}}}}"#, fields.join(","))
    };
    assert!(Mapping::parse_with(many(100).as_bytes(), ON).is_ok());
    let e = Mapping::parse_with(many(101).as_bytes(), ON).expect_err("refused");
    assert_eq!(e.reason, "Limit of nested fields [100] has been exceeded");
}
