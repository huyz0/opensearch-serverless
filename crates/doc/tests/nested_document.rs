//! docs/specs/nested-fields AC-3 to AC-5: a document's nested objects read
//! as child documents.

use oss_doc::document::{self, FieldValue, ParsedDocument};
use oss_doc::mapping::{Mapping, ParseOptions};

const MAPPING: &str = r#"{"properties":{"title":{"type":"keyword"},"tags":{"type":"keyword"},"c":{"type":"nested","properties":{"w":{"type":"keyword"},"n":{"type":"long"},"d":{"type":"nested","properties":{"x":{"type":"long"}}}}}}}"#;

fn mapping() -> Mapping {
    let on = ParseOptions {
        nested: true,
        ..ParseOptions::default()
    };
    Mapping::parse_with(MAPPING.as_bytes(), on).expect("mapping")
}

fn parse(source: &str) -> ParsedDocument {
    document::parse(&mapping(), "1", source.as_bytes()).expect("parses")
}

fn kw(s: &str) -> FieldValue {
    FieldValue::Keyword(s.to_owned())
}

/// A child's path and the names and keyword values of its fields.
fn shown(doc: &ParsedDocument) -> Vec<(String, Vec<String>)> {
    doc.nested
        .iter()
        .map(|n| {
            let fields = n
                .fields
                .iter()
                .map(|(name, v)| match v {
                    FieldValue::Keyword(k) => format!("{name}={k}"),
                    FieldValue::Number(_) => format!("{name}=#"),
                    other => format!("{name}={other:?}"),
                })
                .collect();
            (n.path.clone(), fields)
        })
        .collect()
}

#[test]
fn an_array_of_nested_objects_is_one_child_an_element_and_the_parents_fields_leave_them_out() {
    let doc = parse(r#"{"title":"t","c":[{"w":"a","n":1},{"w":"b"}],"tags":["x","y"]}"#);
    assert_eq!(
        doc.fields,
        [
            ("title".to_owned(), kw("t")),
            ("tags".to_owned(), kw("x")),
            ("tags".to_owned(), kw("y")),
        ]
    );
    assert_eq!(
        shown(&doc),
        [
            ("c".to_owned(), vec!["c.w=a".to_owned(), "c.n=#".to_owned()]),
            ("c".to_owned(), vec!["c.w=b".to_owned()]),
        ]
    );
}

#[test]
fn one_nested_object_is_one_child() {
    let doc = parse(r#"{"c":{"w":"a"}}"#);
    assert_eq!(shown(&doc), [("c".to_owned(), vec!["c.w=a".to_owned()])]);
    assert!(doc.fields.is_empty());
}

#[test]
fn a_nested_object_inside_another_is_written_before_it() {
    let doc = parse(r#"{"c":[{"w":"a","d":[{"x":1},{"x":2}]},{"w":"b"}]}"#);
    let paths: Vec<&str> = doc.nested.iter().map(|n| n.path.as_str()).collect();
    assert_eq!(paths, ["c.d", "c.d", "c", "c"]);
    assert_eq!(
        shown(&doc).get(2),
        Some(&("c".to_owned(), vec!["c.w=a".to_owned()]))
    );
}

#[test]
fn no_object_no_array_and_null_give_no_child() {
    for source in [r#"{}"#, r#"{"c":[]}"#, r#"{"c":null}"#, r#"{"c":[null]}"#] {
        assert!(parse(source).nested.is_empty(), "{source}");
    }
    // An empty object is a child with nothing in it, as the Java writes one.
    assert_eq!(parse(r#"{"c":{}}"#).nested.len(), 1);
}

#[test]
fn a_concrete_value_where_a_nested_object_is_mapped_fails_as_for_an_object() {
    let e = document::parse(&mapping(), "1", br#"{"c":5}"#).expect_err("fails");
    assert!(
        e.reason
            .contains("tried to parse field [c] as object, but found a concrete value"),
        "{}",
        e.reason
    );
}

#[test]
fn more_than_ten_thousand_nested_objects_fail_with_the_javas_message() {
    let children = vec![r#"{"w":"a"}"#; 10_001].join(",");
    let source = format!(r#"{{"c":[{children}]}}"#);
    let e = document::parse(&mapping(), "1", source.as_bytes()).expect_err("fails");
    assert!(
        e.reason.contains(
            "The number of nested documents has exceeded the allowed limit of [10000]. This limit can be set by changing the [index.mapping.nested_objects.limit] index level setting."
        ),
        "{}",
        e.reason
    );
    let ok = format!(r#"{{"c":[{}]}}"#, vec![r#"{"w":"a"}"#; 10_000].join(","));
    assert_eq!(parse(&ok).nested.len(), 10_000);
}

#[test]
fn a_new_field_under_a_nested_object_grows_its_mapping_and_lands_in_the_child() {
    let doc = parse(r#"{"c":[{"w":"a","added":"v","o":{"deep":5}}]}"#);
    let names: Vec<&str> = doc
        .nested
        .iter()
        .flat_map(|n| n.fields.iter().map(|(name, _)| name.as_str()))
        .collect();
    assert_eq!(names, ["c.w", "c.added", "c.added.keyword", "c.o.deep"]);
    let grown = doc.grown.expect("the mapping grew");
    assert!(grown.is_nested("c"), "still nested");
    let leaves: Vec<String> = grown.leaves().into_iter().map(|(n, _)| n).collect();
    assert!(leaves.contains(&"c.added".to_owned()), "{leaves:?}");
    assert!(leaves.contains(&"c.o.deep".to_owned()), "{leaves:?}");
    // The update alone merges into the stored mapping again.
    let update = doc.update.expect("an update");
    assert!(mapping().merge(&update).is_ok());
}
