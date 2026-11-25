//! docs/specs/stock-snapshot-restore AC-3 and AC-9a: `index-N` as OpenSearch
//! wrote it (`fixtures/snapshots/stock-repository-data/GenRepositoryData.java`
//! runs the server's own `RepositoryData.snapshotsToXContent`) reads to its
//! snapshots, indices and metadata blob ids, and a hostile one is refused
//! before any blob key is built from it.

use oss_format::{FormatError, StockRepositoryData, is_blob_name};

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn index_n() -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/stock-repository-data/index-7.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn text() -> String {
    String::from_utf8(index_n()).expect("UTF-8")
}

#[test]
fn the_servers_own_index_blob_reads_to_its_snapshots_and_indices() {
    let data = StockRepositoryData::parse(&index_n()).expect("parses");
    assert_eq!(data.snapshots.len(), 2);
    assert_eq!(data.indices.len(), 2);

    let s1 = data.snapshot("snap-1").expect("snap-1");
    assert_eq!(s1.uuid, "Aa11Bb22Cc33Dd44Ee55Ff");
    assert_eq!(s1.state, Some(1));
    assert_eq!(s1.version.as_deref(), Some("3.10.0"));
    let s2 = data.snapshot("snap-2").expect("snap-2");
    assert!(data.snapshot("snap-3").is_none());

    let names = |s| {
        data.indices_of(s)
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(s1), ["logs", "users"]);
    assert_eq!(names(s2), ["logs"]);

    let logs = data
        .indices
        .iter()
        .find(|i| i.name == "logs")
        .expect("logs");
    assert_eq!(logs.id, "Zq1_aBcD-EfGhIjKlMnOpQ");
    assert_eq!(logs.shard_path_type, 0);
    assert_eq!(
        logs.shard_generations,
        [Some("genLogs0b".to_owned()), Some("genLogs1b".to_owned())]
    );
}

#[test]
fn each_snapshots_metadata_blob_is_the_identifier_of_its_generation() {
    let data = StockRepositoryData::parse(&index_n()).expect("parses");
    let s1 = data.snapshot("snap-1").expect("snap-1");
    let s2 = data.snapshot("snap-2").expect("snap-2");
    let by_name = |n: &str| data.indices.iter().find(|i| i.name == n).expect("index");
    assert_eq!(
        data.metadata_blob_id(s1, by_name("logs")),
        Some("idLogsAaaa")
    );
    assert_eq!(
        data.metadata_blob_id(s2, by_name("logs")),
        Some("idLogsCccc")
    );
    assert_eq!(
        data.metadata_blob_id(s1, by_name("users")),
        Some("idUsersBbbb")
    );
}

#[test]
fn a_repository_from_before_generations_names_the_blob_by_the_snapshot() {
    let json = r#"{"snapshots":[{"name":"old","uuid":"u1","state":1}],
        "indices":{"i":{"id":"x1","snapshots":["u1"],"shard_generations":[null]}}}"#;
    let data = StockRepositoryData::parse(json.as_bytes()).expect("parses");
    let s = data.snapshot("old").expect("old");
    assert_eq!(data.metadata_blob_id(s, &data.indices[0]), Some("u1"));
    assert_eq!(data.indices[0].shard_generations, [None]);
    assert_eq!(data.indices[0].shard_path_type, 0);
}

#[test]
fn a_generation_with_no_identifier_names_no_blob() {
    let json = r#"{"snapshots":[{"name":"n","uuid":"u1","index_metadata_lookup":{"x1":"g"}}],
        "indices":{"i":{"id":"x1","snapshots":["u1"]}},"index_metadata_identifiers":{}}"#;
    let data = StockRepositoryData::parse(json.as_bytes()).expect("parses");
    let s = data.snapshot("n").expect("n");
    assert_eq!(data.metadata_blob_id(s, &data.indices[0]), None);
}

#[test]
fn every_prefix_of_the_blob_is_refused() {
    let bytes = index_n();
    for n in 0..bytes.len() {
        assert!(
            StockRepositoryData::parse(&bytes[..n]).is_err(),
            "a prefix of {n} bytes was accepted"
        );
    }
}

#[test]
fn a_name_that_is_not_safe_in_a_blob_key_is_refused_wherever_it_is_read() {
    let good = text();
    for (from, to) in [
        ("Aa11Bb22Cc33Dd44Ee55Ff\",\"state", "../x\",\"state"), // a snapshot uuid
        ("Zq1_aBcD-EfGhIjKlMnOpQ\",\"shard", "1/../../\",\"shard"), // an index id
        ("\"genLogs0b\"", "\"a/b\""),                           // a shard generation
        ("\"mdLogs2\"", "\"..\""),                              // a metadata generation
        ("\"idLogsCccc\"", "\"x y\""),                          // an identifier
        ("\"mdUsers1\":\"idUsersBbbb\"", "\"\":\"idUsersBbbb\""), // an empty key
    ] {
        let bad = good.replacen(from, to, 1);
        assert_ne!(bad, good, "{from} not found");
        let err = StockRepositoryData::parse(bad.as_bytes()).expect_err(to);
        assert!(matches!(err, FormatError::Corrupt { .. }), "{to}: {err:?}");
    }
    let long = "a".repeat(129);
    let bad = good.replacen("genLogs0b", &long, 1);
    assert!(StockRepositoryData::parse(bad.as_bytes()).is_err());
    let at_limit = good.replacen("genLogs0b", &"a".repeat(128), 1);
    assert!(StockRepositoryData::parse(at_limit.as_bytes()).is_ok());
}

#[test]
fn what_may_be_part_of_a_blob_key() {
    for ok in ["a", "A-b_c.d", "Zq1_aBcD-EfGhIjKlMnOpQ", ".hidden", "..x"] {
        assert!(is_blob_name(ok), "{ok}");
    }
    for bad in ["", ".", "..", "a/b", "a b", "é", "a\0", "a\\b"] {
        assert!(!is_blob_name(bad), "{bad:?}");
    }
}

#[test]
fn an_unknown_top_level_field_and_wrong_shapes_are_refused() {
    let good = text();
    let with_extra = good.replacen("{\"snapshots\"", "{\"surprise\":1,\"snapshots\"", 1);
    assert!(StockRepositoryData::parse(with_extra.as_bytes()).is_err());
    // min_version is read and ignored, as the server does.
    let with_min = good.replacen(
        "{\"snapshots\"",
        "{\"min_version\":\"2.9.0\",\"snapshots\"",
        1,
    );
    assert!(StockRepositoryData::parse(with_min.as_bytes()).is_ok());
    for bad in [
        r#"[]"#,
        r#"{"snapshots":{}}"#,
        r#"{"snapshots":[1]}"#,
        r#"{"snapshots":[{"name":"n"}]}"#,
        r#"{"snapshots":[{"uuid":"u"}]}"#,
        r#"{"snapshots":[{"name":"n","uuid":"u","state":"1"}]}"#,
        r#"{"indices":[]}"#,
        r#"{"indices":{"i":{}}}"#,
        r#"{"indices":{"i":{"id":"x","shard_path_type":"0"}}}"#,
        r#"{"indices":{"i":{"id":"x","snapshots":[1]}}}"#,
        r#"{"indices":{"i":{"id":"x","snapshots":[null]}}}"#,
        r#"{"indices":{"i":{"id":"x","shard_generations":[1]}}}"#,
        r#"{"indices":{"":{"id":"x"}}}"#,
        r#"{"snapshots":[],"indices":{}} trailing"#,
    ] {
        assert!(StockRepositoryData::parse(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn an_index_name_is_at_most_255_bytes() {
    let at = |n: usize| format!(r#"{{"indices":{{"{}":{{"id":"x"}}}}}}"#, "n".repeat(n));
    assert!(StockRepositoryData::parse(at(255).as_bytes()).is_ok());
    assert!(StockRepositoryData::parse(at(256).as_bytes()).is_err());
}

#[test]
fn entries_past_the_budget_are_refused_at_the_count() {
    // The fixture holds 2 snapshots (2 entries and 3 lookups), 2 indices
    // (2 entries, 3 snapshot refs, 3 generations) and 3 identifiers.
    let bytes = index_n();
    let total = 2 + 3 + 2 + 3 + 3 + 3;
    assert!(StockRepositoryData::parse_with(&bytes, total).is_ok());
    assert!(StockRepositoryData::parse_with(&bytes, total - 1).is_err());
}

#[test]
fn a_non_default_shard_path_type_is_kept_for_the_caller_to_refuse() {
    let json = r#"{"indices":{"i":{"id":"x","shard_path_type":2}}}"#;
    let data = StockRepositoryData::parse(json.as_bytes()).expect("parses");
    assert_eq!(data.indices[0].shard_path_type, 2);
}

#[test]
fn nesting_inside_an_unknown_field_is_refused_past_the_readers_depth() {
    let nested = |n: usize| {
        format!(
            r#"{{"snapshots":[{{"name":"n","uuid":"u","extra":{}{}}}]}}"#,
            "[".repeat(n),
            "]".repeat(n)
        )
    };
    assert!(StockRepositoryData::parse(nested(8).as_bytes()).is_ok());
    assert!(StockRepositoryData::parse(nested(100_000).as_bytes()).is_err());
}
