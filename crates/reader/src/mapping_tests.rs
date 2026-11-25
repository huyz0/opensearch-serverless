//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;

fn ty(m: &Mapping, name: &str) -> Option<FieldType> {
    m.get(name).map(|f| f.ty.clone())
}

#[test]
fn types_objects_and_multi_fields() {
    let m = Mapping::from_json(
        r#"{"dynamic":"strict","properties":{"title":{"type":"text","fields":{"raw":{"type":"keyword"}}},
        "year":{"type":"integer"},"n":{"type":"long","index":false},"b":{"type":"byte"},"s":{"type":"short","doc_values":false},
        "when":{"type":"date"},"user":{"properties":{"name":{"type":"keyword"},"meta":{"enabled":false}}},
        "geo":{"type":"geo_point"},"en":{"type":"text","analyzer":"english"},"std":{"type":"text","analyzer":"standard"},
        "lower":{"type":"keyword","normalizer":"lowercase"},"d2":{"type":"date","format":"yyyy"}}}"#,
    )
    .expect("parses");
    assert_eq!(ty(&m, "title"), Some(FieldType::Text));
    assert_eq!(ty(&m, "title.raw"), Some(FieldType::Keyword));
    assert_eq!(ty(&m, "std"), Some(FieldType::Text));
    assert_eq!(
        ty(&m, "year").map(|t| matches!(
            t,
            FieldType::Int {
                name: "integer",
                ..
            }
        )),
        Some(true)
    );
    assert_eq!(
        ty(&m, "b").map(|t| matches!(
            t,
            FieldType::Int {
                min: -128,
                max: 127,
                ..
            }
        )),
        Some(true)
    );
    assert_eq!(
        ty(&m, "s").map(|t| matches!(t, FieldType::Int { name: "short", .. })),
        Some(true)
    );
    assert_eq!(m.get("s").map(|f| f.doc_values), Some(false));
    assert_eq!(ty(&m, "n"), Some(FieldType::Long));
    assert_eq!(m.get("n").map(|f| f.indexed), Some(false));
    assert_eq!(ty(&m, "when"), Some(FieldType::Date));
    assert_eq!(ty(&m, "user.name"), Some(FieldType::Keyword));
    assert_eq!(ty(&m, "user"), None, "an object is not a field");
    assert_eq!(
        ty(&m, "geo"),
        Some(FieldType::Unsupported("a [geo_point] field".into()))
    );
    assert_eq!(
        ty(&m, "en"),
        Some(FieldType::Unsupported(
            "a [text] field with [analyzer]".into()
        ))
    );
    assert!(matches!(ty(&m, "lower"), Some(FieldType::Unsupported(_))));
    assert!(matches!(ty(&m, "d2"), Some(FieldType::Unsupported(_))));
    assert_eq!(m.len(), 13);
    assert!(!m.is_empty());
}

// docs/specs/extension-analyzer AC-13
#[test]
fn a_pinned_analyzer_keeps_the_field_text_and_names_the_pin() {
    let pins = vec!["ext_a".to_owned()];
    let read = |json: &str| Mapping::from_json_pinned(json, &pins).expect("parses");
    let m = read(r#"{"properties":{"t":{"type":"text","analyzer":"ext_a"}}}"#);
    let f = m.get("t").expect("field");
    assert_eq!(f.ty, FieldType::Text);
    assert_eq!(f.analyzer.as_deref(), Some("ext_a"));
    // The same pin as the search analyzer is the same analysis.
    let same = read(
        r#"{"properties":{"t":{"type":"text","analyzer":"ext_a","search_analyzer":"ext_a"}}}"#,
    );
    assert_eq!(same.get("t").expect("t").analyzer.as_deref(), Some("ext_a"));
    // Another search analyzer, a name the index does not pin, a keyword
    // field and an index with no pins are all unsupported.
    for json in [
        r#"{"properties":{"t":{"type":"text","analyzer":"ext_a","search_analyzer":"standard"}}}"#,
        r#"{"properties":{"t":{"type":"text","analyzer":"ext_b"}}}"#,
        r#"{"properties":{"t":{"type":"keyword","analyzer":"ext_a"}}}"#,
    ] {
        let m = read(json);
        assert!(
            matches!(m.get("t").expect("t").ty, FieldType::Unsupported(_)),
            "{json}"
        );
        assert_eq!(m.get("t").expect("t").analyzer, None);
    }
    let none = Mapping::from_json(r#"{"properties":{"t":{"type":"text","analyzer":"ext_a"}}}"#)
        .expect("parses");
    assert!(matches!(
        none.get("t").expect("t").ty,
        FieldType::Unsupported(_)
    ));
}

#[test]
fn the_escaped_form_and_the_typed_form() {
    let m = Mapping::from_escaped(br#"{\"_doc\":{\"properties\":{\"t\":{\"type\":\"text\"}}}}"#)
        .expect("parses");
    assert_eq!(ty(&m, "t"), Some(FieldType::Text));
    assert!(Mapping::from_json("{}").expect("empty").is_empty());
}

#[test]
fn malformed_mappings_are_refused() {
    assert!(Mapping::from_json("[]").is_err());
    assert!(Mapping::from_json(r#"{"properties":{"a":1}}"#).is_err());
    assert!(Mapping::from_json("{").is_err());
    assert!(Mapping::from_escaped(br#"\q"#).is_err());
    let many: Vec<String> = (0..=MAX_FIELDS)
        .map(|i| format!(r#""f{i}":{{"type":"long"}}"#))
        .collect();
    let json = format!(r#"{{"properties":{{{}}}}}"#, many.join(","));
    assert_eq!(
        Mapping::from_json(&json),
        Err(MappingError(format!("more than [{MAX_FIELDS}] fields")))
    );
    let at_limit = format!(r#"{{"properties":{{{}}}}}"#, many[..MAX_FIELDS].join(","));
    assert_eq!(
        Mapping::from_json(&at_limit).map(|m| m.len()),
        Ok(MAX_FIELDS)
    );
    assert_eq!(MappingError("x".into()).to_string(), "mapping: x");
}
