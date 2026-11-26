//! docs/specs/stock-snapshot-export AC-3: an `index-N` with a snapshot added
//! reads back with every earlier snapshot, index id and generation kept, and
//! the new one in the stock field order.

use oss_format::{StockNewIndex, StockNewSnapshot, StockRepositoryData};

const RECORDED: &str = include_str!("../../../fixtures/snapshots/stock/repo/index-0");

fn new<'a>(name: &'a str, uuid: &'a str, indices: &'a [StockNewIndex<'a>]) -> StockNewSnapshot<'a> {
    StockNewSnapshot {
        name,
        uuid,
        state: 1,
        version: "3.10.0",
        indices,
    }
}

fn json(d: &StockRepositoryData) -> String {
    let mut out = Vec::new();
    d.to_json(&mut out).expect("json");
    String::from_utf8(out).unwrap()
}

#[test]
fn the_recorded_index_blob_is_written_back_byte_for_byte() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    assert_eq!(json(&d), RECORDED.trim_end());
}

#[test]
fn a_snapshot_of_a_new_index_is_added_beside_the_recorded_one() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let gens = ["g1".to_owned()];
    let idx = [StockNewIndex {
        name: "metrics",
        id: "ididid",
        metadata_identifier: "ididid-_na_-1-1-1",
        blob_id: "blob1",
        shard_generations: &gens,
    }];
    let next = d.with_snapshot(&new("snap-2", "uuid2", &idx)).unwrap();
    let back = StockRepositoryData::parse(json(&next).as_bytes()).unwrap();
    assert_eq!(back.snapshots.len(), 2);
    assert_eq!(
        back.snapshot("snap-1").unwrap().uuid,
        "q6tlFJQcRUS97oE_eOaE7Q"
    );
    let s2 = back.snapshot("snap-2").unwrap();
    assert_eq!(s2.state, Some(1));
    assert_eq!(s2.version.as_deref(), Some("3.10.0"));
    let names: Vec<_> = back.indices_of(s2).map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["metrics"]);
    let metrics = back.indices.iter().find(|i| i.name == "metrics").unwrap();
    assert_eq!(metrics.shard_generations, [Some("g1".to_owned())]);
    assert_eq!(back.metadata_blob_id(s2, metrics), Some("blob1"));
    let logs = back.indices.iter().find(|i| i.name == "logs").unwrap();
    assert_eq!(logs.id, "M3IuI3tpQ7mcpIwUIA1kfQ");
    assert_eq!(logs.snapshots, ["q6tlFJQcRUS97oE_eOaE7Q"]);
}

#[test]
fn a_second_snapshot_of_a_held_index_joins_it_with_its_new_generations() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let gens = ["g9".to_owned()];
    let idx = [StockNewIndex {
        name: "logs",
        id: "M3IuI3tpQ7mcpIwUIA1kfQ",
        metadata_identifier: "M3IuI3tpQ7mcpIwUIA1kfQ-_na_-1-1-1",
        blob_id: "A8Q5FaIB0HkVpJa_kGxv",
        shard_generations: &gens,
    }];
    let next = d.with_snapshot(&new("snap-2", "uuid2", &idx)).unwrap();
    let back = StockRepositoryData::parse(json(&next).as_bytes()).unwrap();
    let logs = back.indices.iter().find(|i| i.name == "logs").unwrap();
    assert_eq!(logs.snapshots, ["q6tlFJQcRUS97oE_eOaE7Q", "uuid2"]);
    assert_eq!(logs.shard_generations, [Some("g9".to_owned())]);
    assert_eq!(back.indices.len(), 1);
}

#[test]
fn a_held_name_under_another_id_is_refused() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let gens = ["g".to_owned()];
    let idx = [StockNewIndex {
        name: "logs",
        id: "other",
        metadata_identifier: "other-_na_-1-1-1",
        blob_id: "b",
        shard_generations: &gens,
    }];
    assert!(d.with_snapshot(&new("snap-2", "uuid2", &idx)).is_err());
}

#[test]
fn a_repeated_snapshot_name_or_an_unsafe_id_is_refused() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    assert!(d.with_snapshot(&new("snap-1", "uuid2", &[])).is_err());
    assert!(d.with_snapshot(&new("snap-2", "../x", &[])).is_err());
}

#[test]
fn an_empty_repository_starts_from_the_first_snapshot() {
    let d = StockRepositoryData::default();
    let next = d.with_snapshot(&new("only", "u1", &[])).unwrap();
    let back = StockRepositoryData::parse(json(&next).as_bytes()).unwrap();
    assert_eq!(back.snapshots.len(), 1);
    assert!(back.indices.is_empty());
}

/// `OSS_WRITE_STOCK=dir` leaves the `index-N` files for `CheckStockWrite.java`.
#[test]
#[allow(clippy::disallowed_methods)] // ARITH: a test writes a scratch directory.
fn the_written_index_blobs_are_left_for_the_java_check() {
    let Ok(dir) = std::env::var("OSS_WRITE_STOCK") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let gens = ["g1".to_owned(), "g2".to_owned()];
    let idx = [StockNewIndex {
        name: "metrics",
        id: "ididid",
        metadata_identifier: "ididid-_na_-1-1-1",
        blob_id: "blob1",
        shard_generations: &gens,
    }];
    let one = d.with_snapshot(&new("snap-2", "uuid2", &idx)).unwrap();
    let again = [StockNewIndex {
        name: "logs",
        id: "M3IuI3tpQ7mcpIwUIA1kfQ",
        metadata_identifier: "M3IuI3tpQ7mcpIwUIA1kfQ-_na_-1-1-1",
        blob_id: "A8Q5FaIB0HkVpJa_kGxv",
        shard_generations: &["g9".to_owned()],
    }];
    let two = one.with_snapshot(&new("snap-3", "uuid3", &again)).unwrap();
    let empty = StockRepositoryData::default()
        .with_snapshot(&new("only", "u1", &[]))
        .unwrap();
    for (n, data) in [("index-1", &one), ("index-2", &two), ("index-0", &empty)] {
        std::fs::write(format!("{dir}/{n}"), json(data)).unwrap();
    }
}

#[test]
fn a_snapshot_of_two_indices_with_two_shards_each_reads_back_whole() {
    let d = StockRepositoryData::default();
    let gens = ["ga".to_owned(), "gb".to_owned()];
    let idx = [
        StockNewIndex {
            name: "a",
            id: "ida",
            metadata_identifier: "ida-_na_-1-1-1",
            blob_id: "ba",
            shard_generations: &gens,
        },
        StockNewIndex {
            name: "b",
            id: "idb",
            metadata_identifier: "idb-_na_-1-1-1",
            blob_id: "bb",
            shard_generations: &gens,
        },
    ];
    let next = d.with_snapshot(&new("two", "u2", &idx)).unwrap();
    let back = StockRepositoryData::parse(json(&next).as_bytes()).unwrap();
    let s = back.snapshot("two").unwrap();
    assert_eq!(back.indices_of(s).count(), 2);
    for (name, blob) in [("a", "ba"), ("b", "bb")] {
        let i = back.indices.iter().find(|i| i.name == name).unwrap();
        assert_eq!(
            i.shard_generations,
            [Some("ga".to_owned()), Some("gb".to_owned())]
        );
        assert_eq!(back.metadata_blob_id(s, i), Some(blob));
    }
}

#[test]
fn an_unnamed_index_or_an_unsafe_blob_id_is_refused_alone() {
    let d = StockRepositoryData::default();
    let gens = ["g".to_owned()];
    let with = |name, id, ident, blob| {
        let idx = [StockNewIndex {
            name,
            id,
            metadata_identifier: ident,
            blob_id: blob,
            shard_generations: &gens,
        }];
        d.with_snapshot(&new("s", "u", &idx)).is_err()
    };
    assert!(!with("n", "i", "i-_na_-1-1-1", "b"));
    assert!(with("", "i", "i-_na_-1-1-1", "b"));
    assert!(with("n", "../i", "i-_na_-1-1-1", "b"));
    assert!(with("n", "i", "a/b", "b"));
    assert!(with("n", "i", "i-_na_-1-1-1", "b/c"));
}

fn one<'a>(
    name: &'a str,
    id: &'a str,
    ident: &'a str,
    blob: &'a str,
    gens: &'a [String],
) -> StockNewIndex<'a> {
    StockNewIndex {
        name,
        id,
        metadata_identifier: ident,
        blob_id: blob,
        shard_generations: gens,
    }
}

#[test]
fn an_identifier_held_under_another_blob_is_refused_and_under_the_same_is_kept() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let g = ["g".to_owned()];
    let ident = "NMP32gX_STCI-eNifWXUpg-_na_-1-1-1";
    let same = [one(
        "logs",
        "M3IuI3tpQ7mcpIwUIA1kfQ",
        ident,
        "A8Q5FaIB0HkVpJa_kGxv",
        &g,
    )];
    assert!(d.with_snapshot(&new("s2", "u2", &same)).is_ok());
    let other = [one(
        "logs",
        "M3IuI3tpQ7mcpIwUIA1kfQ",
        ident,
        "different",
        &g,
    )];
    assert!(d.with_snapshot(&new("s2", "u2", &other)).is_err());
}

#[test]
fn a_repeated_name_or_id_among_the_new_indices_or_a_held_id_under_a_new_name_is_refused() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let g = ["g".to_owned()];
    let twice = [
        one("a", "ia", "ia-x", "b1", &g),
        one("a", "ib", "ib-x", "b2", &g),
    ];
    assert!(d.with_snapshot(&new("s2", "u2", &twice)).is_err());
    let same_id = [
        one("a", "ia", "ia-x", "b1", &g),
        one("b", "ia", "ia-x", "b1", &g),
    ];
    assert!(d.with_snapshot(&new("s2", "u2", &same_id)).is_err());
    let held_id = [one("renamed", "M3IuI3tpQ7mcpIwUIA1kfQ", "m-x", "b", &g)];
    assert!(d.with_snapshot(&new("s2", "u2", &held_id)).is_err());
}

#[test]
fn an_empty_name_no_shards_or_a_changed_shard_count_is_refused() {
    let d = StockRepositoryData::parse(RECORDED.as_bytes()).unwrap();
    let g = ["g".to_owned()];
    let two = ["g".to_owned(), "h".to_owned()];
    let none: [String; 0] = [];
    let ok = [one("a", "ia", "ia-x", "b", &g)];
    assert!(d.with_snapshot(&new("", "u2", &ok)).is_err());
    let empty = [one("a", "ia", "ia-x", "b", &none)];
    assert!(d.with_snapshot(&new("s2", "u2", &empty)).is_err());
    let ident = "M3IuI3tpQ7mcpIwUIA1kfQ-_na_-1-1-1";
    let resized = [one(
        "logs",
        "M3IuI3tpQ7mcpIwUIA1kfQ",
        ident,
        "A8Q5FaIB0HkVpJa_kGxv",
        &two,
    )];
    assert!(d.with_snapshot(&new("s2", "u2", &resized)).is_err());
}

const UNWRITTEN: &str = r#"{"snapshots":[{"name":"s","uuid":"u","state":1,"index_metadata_lookup":{"i":"i-x"},"version":"3.10.0"}],"indices":{"a":{"id":"i","shard_path_type":0,"snapshots":["u"],"shard_generations":[null]}},"index_metadata_identifiers":{"i-x":"b"}}"#;

#[test]
fn an_unwritten_generation_is_refused_when_adding_and_when_writing() {
    let d = StockRepositoryData::parse(UNWRITTEN.as_bytes()).unwrap();
    let mut out = b"keep".to_vec();
    assert!(d.to_json(&mut out).is_err());
    assert_eq!(out, b"keep");
    assert!(d.with_snapshot(&new("s2", "u2", &[])).is_err());
}
