//! docs/specs/knn-vector-mapping: what the recorded fixtures do not pin: the
//! shapes a caller that does not index `knn_vector` or does not index the
//! field's shape is told are unsupported, never invalid.

use oss_doc::mapping::{Mapping, MappingError, ParseOptions};

const ON: ParseOptions = ParseOptions {
    knn: Some(true),
    nested: false,
};

fn field(extra: &str) -> String {
    format!(r#"{{"properties":{{"v":{{"type":"knn_vector","dimension":3{extra}}}}}}}"#)
}

const HNSW: &str = r#","method":{"name":"hnsw","engine":"lucene"}"#;

fn unsupported(text: &str) -> bool {
    matches!(Mapping::parse_with(text.as_bytes(), ON), Err(e) if e.is_unsupported())
}

#[test]
fn a_caller_that_does_not_ask_for_knn_vector_is_told_it_is_unsupported() {
    let text = field(HNSW);
    let e: MappingError = Mapping::parse(text.as_bytes()).unwrap_err();
    assert!(e.is_unsupported(), "{e:?}");
    assert!(Mapping::parse_with(text.as_bytes(), ON).is_ok());
}

#[test]
fn the_shapes_this_writer_does_not_index_are_unsupported_not_invalid() {
    for extra in [
        // No method: the plugin indexes doc values.
        "",
        // No engine: the plugin resolves Faiss.
        r#","method":{"name":"hnsw"}"#,
        r#","method":{"name":"hnsw","engine":null}"#,
        r#","data_type":"byte","method":{"name":"hnsw","engine":"lucene"}"#,
        r#","data_type":"half_float","method":{"name":"hnsw","engine":"lucene"}"#,
        r#","store":true,"method":{"name":"hnsw","engine":"lucene"}"#,
        r#","doc_values":true,"method":{"name":"hnsw","engine":"lucene"}"#,
        r#","mode":"on_disk","method":{"name":"hnsw","engine":"lucene"}"#,
        r#","compression_level":"16x","method":{"name":"hnsw","engine":"lucene"}"#,
        r#","model_id":"m","method":{"name":"hnsw","engine":"lucene"}"#,
        r#","engine":"lucene","method":{"name":"hnsw","engine":"lucene"}"#,
    ] {
        assert!(unsupported(&field(extra)), "{extra}");
    }
}

#[test]
fn a_field_with_a_method_on_an_index_without_knn_is_refused() {
    let off = ParseOptions {
        knn: Some(false),
        ..ParseOptions::default()
    };
    let e = Mapping::parse_with(field(HNSW).as_bytes(), off).unwrap_err();
    assert!(!e.is_unsupported());
    assert_eq!(
        e.reason,
        "Failed to parse mapping [_doc]: Cannot set modelId or method parameters when index.knn setting is false"
    );
}

#[test]
fn the_method_is_written_with_its_resolved_space_and_reads_back() {
    let text = field(
        r#","method":{"name":"hnsw","engine":"LUCENE","parameters":{"m":8,"ef_construction":64}}"#,
    );
    let mapping = Mapping::parse_with(text.as_bytes(), ON).unwrap();
    let written = mapping.to_json();
    assert_eq!(
        written,
        r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"engine":"lucene","space_type":"l2","name":"hnsw","parameters":{"ef_construction":64,"m":8}}}}}"#
    );
    assert_eq!(
        Mapping::parse_with(written.as_bytes(), ON).unwrap(),
        mapping
    );
}

#[test]
fn the_order_of_the_method_parameters_is_not_part_of_the_mapping() {
    let a = field(
        r#","method":{"name":"hnsw","engine":"lucene","parameters":{"m":8,"ef_construction":64}}"#,
    );
    let b = field(
        r#","method":{"name":"hnsw","engine":"lucene","parameters":{"ef_construction":64,"m":8}}"#,
    );
    assert_eq!(
        Mapping::parse_with(a.as_bytes(), ON).unwrap(),
        Mapping::parse_with(b.as_bytes(), ON).unwrap()
    );
}

#[test]
fn a_method_that_can_be_neither_unsupported_nor_valid_is_refused_in_the_plugins_order() {
    // The space type conflict is found before the engine's own checks.
    let text = field(
        r#","space_type":"l2","method":{"name":"bogus","engine":"lucene","space_type":"cosinesimil"}"#,
    );
    let e = Mapping::parse_with(text.as_bytes(), ON).unwrap_err();
    assert!(
        e.reason
            .ends_with(r#"Cannot specify conflicting space types: "[cosinesimil]" "[l2]""#),
        "{e:?}"
    );
}

#[test]
fn each_unsupported_shape_says_which_it_is() {
    let reason = |extra: &str| {
        Mapping::parse_with(field(extra).as_bytes(), ON)
            .unwrap_err()
            .reason
    };
    assert_eq!(
        reason(r#","method":{"name":"flat"}"#),
        "knn_vector flat method"
    );
    assert_eq!(
        reason(r#","method":{"name":"hnsw"}"#),
        "knn_vector method with no engine (resolved to faiss)"
    );
    assert_eq!(
        reason(r#","method":{"name":"hnsw","engine":"lucene","parameters":{"m":null}}"#),
        "knn_vector null method parameter"
    );
}

#[test]
fn a_method_parameter_must_be_an_integer_that_fits() {
    let reason = |param: &str| {
        let text = format!(
            r#"{{"properties":{{"v":{{"type":"knn_vector","dimension":3,"method":{{"name":"hnsw","engine":"lucene","parameters":{{"m":{param}}}}}}}}}}}"#
        );
        Mapping::parse_with(text.as_bytes(), ON).unwrap_err().reason
    };
    let reason = |param: &str| reason(param).replace("Failed to parse mapping [_doc]: ", "");
    let not_integer =
        "Validation Failed: 1: value is not an instance of Integer for Integer parameter [m].;";
    assert_eq!(reason("3000000000"), not_integer);
    assert_eq!(reason("1.5"), not_integer);
    assert_eq!(
        reason("0"),
        "Validation Failed: 1: parameter validation failed for Integer parameter [m].;"
    );
}

#[test]
fn a_method_without_a_dimension_is_told_the_dimension_is_missing() {
    let text =
        r#"{"properties":{"v":{"type":"knn_vector","method":{"name":"hnsw","engine":"lucene"}}}}"#;
    let e = Mapping::parse_with(text.as_bytes(), ON).unwrap_err();
    assert_eq!(
        e.reason,
        "Failed to parse mapping [_doc]: Dimension value missing for vector: v"
    );
}

#[test]
fn a_stored_mapping_with_a_vector_field_is_read_whatever_the_setting() {
    let text = r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"name":"hnsw","engine":"lucene","space_type":"l2"}}}}"#;
    assert!(Mapping::parse(text.as_bytes()).is_err());
    assert!(Mapping::from_descriptor(Some(text)).is_ok());
}
