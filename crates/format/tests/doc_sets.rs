//! Document sets (templates and pipelines) read and written as the Java's
//! `TemplateStore` does (docs/specs/doc-sets/SPEC.md, AC-1 and AC-2).

use oss_format::{
    DocKind, DocSetMarker, FormatError, MAX_DOC_SET_MARKER_BYTES, MAX_DOCS, doc_content_key,
    doc_set_marker_key,
};

// What `TemplateStore.writeMarker` writes for two entries: `XContentBuilder`
// output, compact, fields in `TreeMap` order.
const JAVA_MARKER: &str = r#"{"version":3,"entries":{"logs":"0a1b-AbCdEfGhIjKlMnOpQrStUv","metrics":"0a1b-AbCdEfGhIjKlMnOpQrStUv"}}"#;

fn written(marker: &DocSetMarker<'_>) -> String {
    let mut buf = vec![0u8; MAX_DOC_SET_MARKER_BYTES];
    let n = marker.write(&mut buf).unwrap();
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

#[test]
fn the_javas_marker_reads_and_writes_the_same_bytes() {
    let marker = DocSetMarker::parse(JAVA_MARKER.as_bytes()).unwrap();
    assert_eq!(marker.version, 3);
    let names: Vec<&str> = marker.entries.iter().map(|(n, _)| n.as_ref()).collect();
    assert_eq!(names, ["logs", "metrics"]);
    assert_eq!(written(&marker), JAVA_MARKER);
}

#[test]
fn entries_are_written_in_the_javas_tree_map_order() {
    let mut marker = DocSetMarker::new(7);
    // UTF-16 order, as `String.compareTo`: U+FF5E sorts above U+1F600's
    // surrogate pair, though its UTF-8 bytes sort below.
    marker.insert("\u{ff5e}", "k3");
    marker.insert("\u{1f600}", "k2");
    marker.insert("b", "k1");
    marker.insert("B", "k0");
    assert_eq!(
        written(&marker),
        "{\"version\":7,\"entries\":{\"B\":\"k0\",\"b\":\"k1\",\"\u{1f600}\":\"k2\",\"\u{ff5e}\":\"k3\"}}"
    );
}

#[test]
fn an_empty_marker_has_no_entries() {
    let marker = DocSetMarker::new(1);
    assert_eq!(written(&marker), r#"{"version":1,"entries":{}}"#);
    assert_eq!(
        DocSetMarker::parse(br#"{"version":1,"entries":{}}"#).unwrap(),
        marker
    );
}

#[test]
fn a_missing_version_or_entries_reads_as_the_java_reads_it() {
    let marker = DocSetMarker::parse(br#"{"entries":{"a":"k"}}"#).unwrap();
    assert_eq!(marker.version, 0);
    assert_eq!(marker.entries.len(), 1);
    let marker = DocSetMarker::parse(br#"{"version":4}"#).unwrap();
    assert_eq!((marker.version, marker.entries.len()), (4, 0));
    let marker = DocSetMarker::parse(br#"{"version":4,"other":[1,{"x":2}],"entries":{}}"#).unwrap();
    assert_eq!(marker.version, 4);
}

#[test]
fn a_counter_from_before_the_register_held_entries_is_unsupported() {
    assert!(matches!(
        DocSetMarker::parse(b"12\n"),
        Err(FormatError::Unsupported { version: 12, .. })
    ));
    assert!(matches!(
        DocSetMarker::parse(b"not a number"),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_marker_the_java_could_not_have_written_is_refused() {
    for bad in [
        &br#"{"version":"3","entries":{}}"#[..],
        br#"{"version":1,"entries":{"a":7}}"#,
        br#"{"version":1,"entries":[]}"#,
        br#"{"version":1,"entries":{"a":"k/b"}}"#,
        br#"{"version":1,"entries":{"a":""}}"#,
        br#"{"version":1,"entries":{"a":".."}}"#,
        br#"{"version":1,"version":2}"#,
        br#"{"version":1,"entries":{}} x"#,
        br#"{"version":1,"entries":{}"#,
        b"",
    ] {
        assert!(
            DocSetMarker::parse(bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn more_entries_than_the_cap_are_refused_in_both_directions() {
    let mut marker = DocSetMarker::new(1);
    for i in 0..=MAX_DOCS {
        marker.insert(&format!("t{i}"), "k");
    }
    let mut buf = vec![0u8; MAX_DOC_SET_MARKER_BYTES];
    assert!(matches!(
        marker.write(&mut buf),
        Err(FormatError::TooLarge { .. })
    ));
    let mut text = String::from(r#"{"version":1,"entries":{"#);
    for i in 0..=MAX_DOCS {
        if i > 0 {
            text.push(',');
        }
        text.push_str(&format!("\"t{i}\":\"k\""));
    }
    text.push_str("}}");
    assert!(matches!(
        DocSetMarker::parse(text.as_bytes()),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn the_cap_of_entries_at_their_longest_fits_the_bound() {
    let mut marker = DocSetMarker::new(i64::MAX);
    let key = format!("{}-{}", "f".repeat(64), "A".repeat(22));
    for i in 0..MAX_DOCS {
        marker.insert(&format!("{i:0>255}"), &key);
    }
    let text = written(&marker);
    assert!(text.len() <= MAX_DOC_SET_MARKER_BYTES, "{}", text.len());
    assert_eq!(DocSetMarker::parse(text.as_bytes()).unwrap(), marker);
}

#[test]
fn a_short_buffer_is_refused() {
    let marker = DocSetMarker::parse(JAVA_MARKER.as_bytes()).unwrap();
    assert!(matches!(
        marker.write(&mut [0u8; 10]),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn keys_are_the_javas_register_map_paths() {
    for (kind, dir) in [
        (DocKind::IndexTemplate, "templates/index/"),
        (DocKind::ComponentTemplate, "templates/component/"),
        (DocKind::IngestPipeline, "pipelines/"),
        (DocKind::SearchPipeline, "search_pipelines/"),
    ] {
        assert_eq!(
            doc_set_marker_key("d/", kind).unwrap(),
            format!("d/{dir}_version")
        );
        assert_eq!(
            doc_content_key("", kind, "x-1").unwrap(),
            format!("{dir}blob-x-1")
        );
    }
}

#[test]
fn a_key_is_refused_for_a_bad_base_or_content() {
    assert!(doc_set_marker_key("d", DocKind::IndexTemplate).is_err());
    assert!(doc_content_key("d", DocKind::IndexTemplate, "x").is_err());
    for content in ["", "..", ".", "a/b"] {
        assert!(
            doc_content_key("d/", DocKind::IngestPipeline, content).is_err(),
            "{content}"
        );
    }
}

#[test]
fn the_document_bound_is_a_mebibyte() {
    assert_eq!(oss_format::MAX_DOC_BYTES, 1_048_576);
}

#[test]
fn a_marker_names_replaces_removes_and_shares() {
    let mut marker = DocSetMarker::new(1);
    assert_eq!(marker.content("a"), None);
    marker.insert("a", "k1");
    marker.insert("b", "k1");
    marker.insert("c", "k2");
    assert_eq!(marker.content("a"), Some("k1"));
    assert_eq!(marker.content("c"), Some("k2"));
    assert!(marker.shares("k1") && marker.shares("k2") && !marker.shares("k3"));
    marker.insert("c", "k3");
    assert_eq!(marker.entries.len(), 3);
    assert_eq!(marker.content("c"), Some("k3"));
    assert!(!marker.shares("k2"));
    assert_eq!(marker.remove("a").as_deref(), Some("k1"));
    assert_eq!(marker.remove("a"), None);
    assert!(marker.shares("k1"));
    assert_eq!(marker.content("b"), Some("k1"));
}

#[test]
fn the_learning_to_rank_sets_have_their_own_directories_and_a_model_may_be_larger() {
    assert_eq!(
        doc_set_marker_key("", DocKind::LtrStores).unwrap(),
        "ltr/stores/_version"
    );
    assert_eq!(
        doc_set_marker_key("ltr/shop/", DocKind::LtrFeatureSets).unwrap(),
        "ltr/shop/featuresets/_version"
    );
    assert_eq!(
        doc_content_key("ltr/shop/", DocKind::LtrModels, "k").unwrap(),
        "ltr/shop/models/blob-k"
    );
    assert_eq!(
        DocKind::LtrFeatureSets.max_doc_bytes(),
        oss_format::MAX_DOC_BYTES
    );
    assert_eq!(
        DocKind::LtrModels.max_doc_bytes(),
        oss_format::MAX_MODEL_BYTES
    );
    assert_eq!(oss_format::MAX_MODEL_BYTES, 10 << 20);
}

#[test]
fn the_ism_policies_have_their_own_directory_and_the_common_bound() {
    assert_eq!(
        doc_set_marker_key("", DocKind::IsmPolicies).unwrap(),
        "ism/policies/_version"
    );
    assert_eq!(
        doc_content_key("", DocKind::IsmPolicies, "k").unwrap(),
        "ism/policies/blob-k"
    );
    assert_eq!(
        DocKind::IsmPolicies.max_doc_bytes(),
        oss_format::MAX_DOC_BYTES
    );
}

#[test]
fn the_ml_model_groups_have_their_own_directory_and_the_common_bound() {
    assert_eq!(
        doc_set_marker_key("", DocKind::MlModelGroups).unwrap(),
        "ml/model_groups/_version"
    );
    assert_eq!(
        DocKind::MlModelGroups.max_doc_bytes(),
        oss_format::MAX_DOC_BYTES
    );
}
