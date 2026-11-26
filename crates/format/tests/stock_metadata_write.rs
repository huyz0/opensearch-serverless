//! docs/specs/stock-snapshot-export AC-4 (the snapshot info, global metadata
//! and index metadata writers): each writer, given the recorded stock
//! snapshot's inputs, writes the JSON the recorded blob decodes to, and the
//! blob it makes in the envelope reads back through the restore's readers.

use oss_format::{
    StockEnvelopeLimits, StockIndexMetadata, StockNewIndexMetadata, StockNewSnapshotInfo,
    StockSnapshotInfo, stock_blob_from_json, stock_blob_to_json, stock_global_metadata_json,
    stock_index_metadata_json, stock_snapshot_info_json,
};

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn recorded(file: &str, codec: &str) -> String {
    let path = format!(
        "{}/../../fixtures/snapshots/stock/repo/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    let blob = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut out = Vec::new();
    stock_blob_to_json(&blob, codec, &StockEnvelopeLimits::default(), &mut out).expect("decodes");
    String::from_utf8(out).expect("UTF-8")
}

fn text(f: impl FnOnce(&mut Vec<u8>) -> Result<(), oss_format::FormatError>) -> String {
    let mut out = Vec::new();
    f(&mut out).expect("written");
    String::from_utf8(out).expect("UTF-8")
}

const LOGS_MAPPING: &str =
    r#"{"properties":{"level":{"type":"keyword"},"n":{"type":"integer"},"title":{"type":"text"}}}"#;

fn logs<'a>(
    settings: &'a [(String, String)],
    allocations: &'a [String],
) -> StockNewIndexMetadata<'a> {
    StockNewIndexMetadata {
        name: "logs",
        uuid: "NMP32gX_STCI-eNifWXUpg",
        number_of_shards: 1,
        creation_date_ms: 1_795_652_423_002,
        mapping: Some(LOGS_MAPPING),
        settings,
        allocation_ids: allocations,
    }
}

#[test]
fn the_global_metadata_is_the_recorded_empty_one() {
    assert_eq!(
        text(stock_global_metadata_json),
        recorded("meta-q6tlFJQcRUS97oE_eOaE7Q.dat", "metadata")
    );
}

#[test]
fn the_snapshot_info_is_the_recorded_one() {
    let new = StockNewSnapshotInfo {
        name: "snap-1",
        uuid: "q6tlFJQcRUS97oE_eOaE7Q",
        indices: &["logs"],
        start_time_ms: 1_795_652_423_613,
        end_time_ms: 1_795_652_423_613,
        total_shards: 1,
        successful_shards: 1,
    };
    let got = text(|o| stock_snapshot_info_json(&new, o));
    assert_eq!(got, recorded("snap-q6tlFJQcRUS97oE_eOaE7Q.dat", "snapshot"));
    let read = StockSnapshotInfo::parse(got.as_bytes()).unwrap();
    assert_eq!(read.indices, ["logs"]);
    assert_eq!(read.include_global_state, Some(false));
}

#[test]
fn the_index_metadata_is_the_recorded_one() {
    let alloc = ["ixTCGcPhQ7K66BJJ_rGLAA".to_owned()];
    let got = text(|o| stock_index_metadata_json(&logs(&[], &alloc), o));
    let want = recorded(
        "indices/M3IuI3tpQ7mcpIwUIA1kfQ/meta-A8Q5FaIB0HkVpJa_kGxv.dat",
        "index-metadata",
    );
    assert_eq!(got, want);
}

#[test]
fn the_index_metadata_reads_back_with_its_mapping_and_settings() {
    let alloc = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
    let extra = [
        ("index.refresh_interval".to_owned(), "5s".to_owned()),
        // The node's own keys are not the caller's to set.
        ("index.number_of_replicas".to_owned(), "9".to_owned()),
        ("index.uuid".to_owned(), "x".to_owned()),
    ];
    let mut new = logs(&extra, &alloc);
    new.number_of_shards = 3;
    let got = text(|o| stock_index_metadata_json(&new, o));
    let read = StockIndexMetadata::parse(got.as_bytes()).unwrap();
    assert_eq!(read.mapping.as_deref(), Some(LOGS_MAPPING));
    assert_eq!(read.setting("index.number_of_shards"), Some("3"));
    assert_eq!(read.setting("index.number_of_replicas"), Some("0"));
    assert_eq!(read.setting("index.uuid"), Some("NMP32gX_STCI-eNifWXUpg"));
    assert_eq!(read.setting("index.refresh_interval"), Some("5s"));
    assert_eq!(read.state, "open");
    assert!(got.contains(r#""primary_terms":[1,1,1]"#), "{got}");
    assert!(got.contains(r#""active_shard_ids":[0,1,2]"#), "{got}");
    assert!(got.contains(r#""routing_num_shards":768"#), "{got}");
}

#[test]
fn an_index_with_no_mapping_writes_none() {
    let alloc = ["a".to_owned()];
    let mut new = logs(&[], &alloc);
    new.mapping = None;
    let got = text(|o| stock_index_metadata_json(&new, o));
    assert!(got.contains(r#""mappings":[]"#), "{got}");
    assert_eq!(
        StockIndexMetadata::parse(got.as_bytes()).unwrap().mapping,
        None
    );
}

#[test]
fn a_bad_index_is_refused_and_the_buffer_keeps_its_start() {
    let alloc = ["a".to_owned()];
    let none: [String; 0] = [];
    let cases: Vec<(&str, StockNewIndexMetadata<'_>)> = vec![
        (
            "no shards",
            StockNewIndexMetadata {
                number_of_shards: 0,
                ..logs(&[], &alloc)
            },
        ),
        (
            "too many shards",
            StockNewIndexMetadata {
                number_of_shards: 4097,
                ..logs(&[], &alloc)
            },
        ),
        (
            "allocations short",
            StockNewIndexMetadata {
                allocation_ids: &none,
                ..logs(&[], &alloc)
            },
        ),
        (
            "empty name",
            StockNewIndexMetadata {
                name: "",
                ..logs(&[], &alloc)
            },
        ),
        (
            "unsafe uuid",
            StockNewIndexMetadata {
                uuid: "a/b",
                ..logs(&[], &alloc)
            },
        ),
        (
            "a mapping that splices",
            StockNewIndexMetadata {
                mapping: Some(r#"{}}],"x":[{"a":{}"#),
                ..logs(&[], &alloc)
            },
        ),
        (
            "a mapping that is not an object",
            StockNewIndexMetadata {
                mapping: Some("[1]"),
                ..logs(&[], &alloc)
            },
        ),
    ];
    for (why, new) in cases {
        let mut out = b"keep".to_vec();
        assert!(stock_index_metadata_json(&new, &mut out).is_err(), "{why}");
        assert_eq!(out, b"keep", "{why}");
    }
}

#[test]
fn a_written_blob_in_the_envelope_reads_back_equal() {
    let alloc = ["ixTCGcPhQ7K66BJJ_rGLAA".to_owned()];
    let json = text(|o| stock_index_metadata_json(&logs(&[], &alloc), o));
    let mut blob = Vec::new();
    stock_blob_from_json(
        json.as_bytes(),
        "index-metadata",
        &StockEnvelopeLimits::default(),
        &mut blob,
    )
    .unwrap();
    let mut back = Vec::new();
    stock_blob_to_json(
        &blob,
        "index-metadata",
        &StockEnvelopeLimits::default(),
        &mut back,
    )
    .unwrap();
    assert_eq!(String::from_utf8(back).unwrap(), json);
}

/// `OSS_WRITE_STOCK=dir` leaves the blobs for `CheckStockWrite.java`.
#[test]
#[allow(clippy::disallowed_methods)] // ARITH: a test writes a scratch directory.
fn the_metadata_blobs_are_left_for_the_java_check() {
    let Ok(dir) = std::env::var("OSS_WRITE_STOCK") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let put = |name: &str, codec: &str, json: &str| {
        let mut blob = Vec::new();
        stock_blob_from_json(
            json.as_bytes(),
            codec,
            &StockEnvelopeLimits::default(),
            &mut blob,
        )
        .unwrap();
        std::fs::write(format!("{dir}/{name}"), blob).unwrap();
    };
    let alloc = ["ixTCGcPhQ7K66BJJ_rGLAA".to_owned()];
    let extra = [("index.refresh_interval".to_owned(), "5s".to_owned())];
    put(
        "index-metadata.dat",
        "index-metadata",
        &text(|o| stock_index_metadata_json(&logs(&extra, &alloc), o)),
    );
    let three = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
    let mut many = logs(&[], &three);
    many.number_of_shards = 3;
    put(
        "index-metadata-3.dat",
        "index-metadata",
        &text(|o| stock_index_metadata_json(&many, o)),
    );
    put(
        "global-meta.dat",
        "metadata",
        &text(stock_global_metadata_json),
    );
    let new = StockNewSnapshotInfo {
        name: "snap-1",
        uuid: "q6tlFJQcRUS97oE_eOaE7Q",
        indices: &["logs"],
        start_time_ms: 1,
        end_time_ms: 2,
        total_shards: 1,
        successful_shards: 1,
    };
    put(
        "snapshot-info.dat",
        "snapshot",
        &text(|o| stock_snapshot_info_json(&new, o)),
    );
}

#[test]
fn the_shard_count_bounds_hold_at_the_limit_and_one_past() {
    let ids = |n: usize| (0..n).map(|i| format!("a{i}")).collect::<Vec<_>>();
    let at = |shards: u32, n: usize| {
        let alloc = ids(n);
        let new = StockNewIndexMetadata {
            number_of_shards: shards,
            ..logs(&[], &alloc)
        };
        stock_index_metadata_json(&new, &mut Vec::new()).is_ok()
    };
    assert!(!at(0, 0));
    assert!(at(1, 1));
    assert!(at(4096, 4096));
    assert!(!at(4097, 4097));
}

#[test]
fn a_snapshot_needs_a_name_and_a_safe_uuid_each_and_lists_every_index() {
    let with = |name, uuid, indices: &[&str]| {
        let new = StockNewSnapshotInfo {
            name,
            uuid,
            indices,
            start_time_ms: 1,
            end_time_ms: 2,
            total_shards: 1,
            successful_shards: 1,
        };
        let mut out = Vec::new();
        stock_snapshot_info_json(&new, &mut out).map(|()| String::from_utf8(out).unwrap())
    };
    assert!(with("", "u", &["a"]).is_err());
    assert!(with("n", "a/b", &["a"]).is_err());
    assert!(with("n", "", &["a"]).is_err());
    let two = with("n", "u", &["a", "b"]).unwrap();
    assert!(two.contains(r#""indices":["a","b"]"#), "{two}");
}
