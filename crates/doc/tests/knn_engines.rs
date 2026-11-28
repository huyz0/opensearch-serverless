//! docs/specs/knn-plugin-api: the `faiss` engine's float
//! HNSW method is accepted and served by the Lucene HNSW codec, and every
//! refusal the stock plugin makes for the engines is made here. Shapes are
//! from the stock 3.10.0 recordings in `fixtures/compat/knn-options`.

use oss_doc::mapping::{Mapping, ParseOptions};

const ON: ParseOptions = ParseOptions {
    knn: Some(true),
    nested: false,
};

fn parse(extra: &str, dimension: u32) -> Result<Mapping, oss_doc::mapping::MappingError> {
    let text = format!(
        r#"{{"properties":{{"v":{{"type":"knn_vector","dimension":{dimension}{extra}}}}}}}"#
    );
    Mapping::parse_with(text.as_bytes(), ON)
}

fn faiss(params: &str) -> String {
    format!(r#","method":{{"name":"hnsw","engine":"faiss","space_type":"l2"{params}}}"#)
}

fn reason(extra: &str) -> String {
    parse(extra, 3).unwrap_err().reason
}

#[test]
fn faiss_hnsw_is_accepted_and_written_back_with_its_engine() {
    let m = parse(&faiss(""), 3).unwrap().to_json();
    assert_eq!(
        m,
        r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"engine":"faiss","space_type":"l2","name":"hnsw","parameters":{}}}}}"#
    );
    assert_eq!(Mapping::parse_with(m.as_bytes(), ON).unwrap().to_json(), m);
}

#[test]
fn faiss_parameters_are_written_in_the_plugins_order() {
    let p = r#","parameters":{"m":8,"ef_construction":100,"ef_search":50}"#;
    assert_eq!(
        parse(&faiss(p), 3).unwrap().to_json(),
        r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"engine":"faiss","space_type":"l2","name":"hnsw","parameters":{"ef_search":50,"ef_construction":100,"m":8}}}}}"#
    );
}

#[test]
fn the_flat_encoder_is_accepted_and_written_with_empty_parameters() {
    let p = r#","parameters":{"encoder":{"name":"flat"}}"#;
    let m = parse(&faiss(p), 3).unwrap().to_json();
    assert!(
        m.contains(r#""parameters":{"encoder":{"name":"flat","parameters":{}}}"#),
        "{m}"
    );
}

#[test]
fn faiss_refusals_carry_the_stock_messages() {
    let spaces = |s: &str| {
        reason(&format!(
            r#","method":{{"name":"hnsw","engine":"faiss","space_type":"{s}"}}"#
        ))
    };
    assert!(
        spaces("l1").ends_with(
            r#"Validation Failed: 1: "hnsw" with "faiss" configuration does not support space type: "l1".;"#
        )
    );
    assert!(spaces("linf").contains(r#"space type: "linf""#));
    let enc = |e: &str| reason(&faiss(&format!(r#","parameters":{{"encoder":{e}}}"#)));
    let bits = "Parameter [bits] is required for encoder [sq] on indices created with version 3.6.0 or later. Supported values: [2, 1, 16, 4];";
    assert!(enc(r#"{"name":"sq","parameters":{"type":"fp16"}}"#).ends_with(bits));
    assert!(
        enc(r#"{"name":"pq","parameters":{"m":1,"code_size":8}}"#)
            .ends_with(r#"Validation Failed: 1: "method" requires training.;"#)
    );
    assert!(
        reason(&faiss(r#","parameters":{"nope":1}"#)).ends_with("Unknown parameter 'nope' found;")
    );
    assert!(
        reason(&faiss(r#","parameters":{"m":0}"#))
            .ends_with("parameter validation failed for Integer parameter [m].;")
    );
    assert!(
        reason(&faiss(r#","parameters":{"m":"x"}"#))
            .ends_with("value is not an instance of Integer for Integer parameter [m].;")
    );
    let ivf = reason(
        r#","method":{"name":"ivf","engine":"faiss","space_type":"l2","parameters":{"nlist":4}}"#,
    );
    assert!(
        ivf.ends_with(r#"Validation Failed: 1: "method" requires training.;"#),
        "{ivf}"
    );
}

#[test]
fn faiss_accepts_a_large_m_and_the_dimension_limit_is_16000() {
    assert!(parse(&faiss(r#","parameters":{"m":100000}"#), 3).is_ok());
    assert!(parse(&faiss(""), 16_000).is_ok());
    assert!(
        parse(&faiss(""), 16_001).unwrap_err().reason.ends_with(
            "Dimension value cannot be greater than 16000 for vector with engine: faiss;"
        )
    );
}

#[test]
fn nmslib_is_refused_for_a_new_index_as_the_stock_plugin_refuses_it() {
    let r = reason(r#","method":{"name":"hnsw","engine":"nmslib","space_type":"l2"}"#);
    assert_eq!(
        r,
        "Failed to parse mapping [_doc]: nmslib engine is deprecated in OpenSearch  and cannot be used for new index creation in OpenSearch from  3.0.0."
    );
}

#[test]
fn lucene_still_refuses_ef_search() {
    let r = reason(
        r#","method":{"name":"hnsw","engine":"lucene","space_type":"l2","parameters":{"ef_search":10}}"#,
    );
    assert!(r.ends_with("Unknown parameter 'ef_search' found;"), "{r}");
}
