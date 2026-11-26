//! docs/specs/knn-vector-document: the values a `knn_vector` field reads,
//! which the recorded fixtures (replayed in `mapping_fixtures.rs`) do not
//! show: what is read, and the shapes left to a Java writer.

use oss_doc::document::{self, FieldValue};
use oss_doc::mapping::{Mapping, ParseOptions};

const MAPPING: &str = r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"name":"hnsw","engine":"lucene"}},"k":{"type":"keyword"},"o":{"properties":{"v":{"type":"knn_vector","dimension":2,"method":{"name":"hnsw","engine":"lucene"}}}}}}"#;

fn mapping() -> Mapping {
    Mapping::parse_with(
        MAPPING.as_bytes(),
        ParseOptions {
            knn: Some(true),
            ..ParseOptions::default()
        },
    )
    .unwrap()
}

fn vector(doc: &str, name: &str) -> Option<Vec<f32>> {
    let parsed = document::parse(&mapping(), "1", doc.as_bytes()).unwrap();
    parsed.fields.into_iter().find_map(|(n, v)| match v {
        FieldValue::Vector(c) if n == name => Some(c),
        _ => None,
    })
}

#[test]
fn a_vector_is_read_beside_other_fields_and_inside_an_object() {
    assert_eq!(
        vector(r#"{"k":"a","v":[1,2,3],"o":{"v":[4,5]}}"#, "v"),
        Some(vec![1.0, 2.0, 3.0])
    );
    assert_eq!(
        vector(r#"{"k":"a","v":[1,2,3],"o":{"v":[4,5]}}"#, "o.v"),
        Some(vec![4.0, 5.0])
    );
    let doc = document::parse(&mapping(), "1", br#"{"v":[1,2,3],"k":"a"}"#).unwrap();
    assert_eq!(doc.fields.len(), 2);
}

#[test]
fn a_null_vector_adds_nothing() {
    assert_eq!(vector(r#"{"v":null,"k":"a"}"#, "v"), None);
    assert_eq!(vector(r#"{"k":"a"}"#, "v"), None);
}

#[test]
fn a_component_is_the_float_java_reads() {
    let got = vector(r#"{"v":[-0.0,1e-45,0.12345678901234567]}"#, "v").unwrap();
    assert_eq!(got[0].to_bits(), (-0.0_f32).to_bits());
    assert_eq!(got[1], f32::from_bits(1));
    assert_eq!(got[2], 0.123_456_79_f32);
    // A string reads as `Float.parseFloat` does: trimmed, with a suffix.
    assert_eq!(
        vector(r#"{"v":[" 1 ","2f","3.5"]}"#, "v"),
        Some(vec![1.0, 2.0, 3.5])
    );
}

#[test]
fn the_shapes_left_to_a_java_writer_are_unsupported_not_invalid() {
    for doc in [
        // Valid base64 is another encoding of the vector.
        r#"{"v":"AAAAAAAAAAAAAAAA"}"#,
        r#"{"v":true}"#,
        r#"{"v":[{"a":1},2,3]}"#,
    ] {
        let e = document::parse(&mapping(), "1", doc.as_bytes()).unwrap_err();
        let mut cause = Some(&e);
        let mut unsupported = false;
        while let Some(c) = cause {
            unsupported |= c.is_unsupported();
            cause = c.caused_by.as_deref();
        }
        assert!(unsupported, "{doc}: {e:?}");
    }
}

#[test]
fn an_infinity_is_previewed_with_its_sign() {
    for (doc, shown) in [
        (r#"{"v":[1e39,1,1]}"#, "Infinity"),
        (r#"{"v":[-1e39,1,1]}"#, "-Infinity"),
    ] {
        let e = document::parse(&mapping(), "1", doc.as_bytes()).unwrap_err();
        assert!(
            e.reason
                .ends_with(&format!("Preview of field's value: '{shown}'")),
            "{e:?}"
        );
    }
}
