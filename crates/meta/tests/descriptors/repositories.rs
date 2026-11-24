//! docs/specs/repository-write AC-1, AC-2, AC-4, AC-5: registering and
//! removing a repository, per backend, with exact request counts.

use std::path::PathBuf;

use oss_blob::S3Op::{Delete, Head, List, Put};
use oss_blob::SliceSource;
use oss_format::{RepositoryRecord, sha256_hex, snapshot_state_key};
use oss_meta::{
    MAX_SNAPSHOT_MARKERS, Removal, RepositoryWriteError, RepositoryWriter, SnapshotReader,
};

use crate::{BASE, Backend, all, counted};

const ID: &str = "gen0000000000000000001";

#[allow(clippy::disallowed_methods)] // a fixture a few hundred bytes long
fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn put(b: &Backend, key: &str, body: &[u8]) {
    b.store
        .put(
            &format!("{BASE}{key}"),
            &mut SliceSource::new(body),
            &mut [0u8; 256],
        )
        .unwrap();
}

fn writer(b: &Backend) -> RepositoryWriter {
    RepositoryWriter::new(std::sync::Arc::clone(&b.store), BASE)
}

fn record(name: &str, created: i64) -> RepositoryRecord {
    RepositoryRecord {
        name: name.to_owned(),
        repo_type: "fs".to_owned(),
        created_at_millis: created,
        settings: vec![("location".to_owned(), "x".to_owned())],
    }
}

fn vint(out: &mut Vec<u8>, mut n: usize) {
    while n >= 0x80 {
        out.push(u8::try_from(n & 0x7f).unwrap() | 0x80);
        n >>= 7;
    }
    out.push(u8::try_from(n).unwrap());
}

/// The current-layout marker of `repo`/`name`, and the register value that
/// holds the fixture's snapshot under it.
fn held(repo: &str, name: &str) -> (String, Vec<u8>) {
    let marker = format!(
        "v2-{}#{}#{ID}",
        sha256_hex(repo),
        sha256_hex(&format!("{repo}#{name}"))
    );
    let record = String::from_utf8(fixture("snapshot-finished.json"))
        .unwrap()
        .replace("\"backups\"", &format!("\"{repo}\""))
        .replace("\"daily-1\"", &format!("\"{name}\""));
    let mut value = Vec::new();
    vint(&mut value, marker.len());
    value.extend_from_slice(marker.as_bytes());
    vint(&mut value, record.len());
    value.extend_from_slice(record.as_bytes());
    value.push(1);
    (marker, value)
}

fn register(b: &Backend, repo: &str, name: &str, value: &[u8]) {
    let key = snapshot_state_key(BASE, repo, name).unwrap();
    b.store.create_register(&key, value).unwrap();
}

fn registered(b: &Backend, name: &str) -> bool {
    let mut buf = vec![0u8; 4096];
    SnapshotReader::new(std::sync::Arc::clone(&b.store), BASE)
        .repository(name, &mut buf)
        .unwrap()
        .is_some()
}

#[test]
fn a_repository_is_one_put_an_upsert_and_reads_back() {
    for b in all() {
        let w = writer(&b);
        let mut buf = vec![0u8; 4096];
        let (_, n) = counted(b.meter(), || {
            w.put(&record("backups", 1), &mut buf).unwrap()
        });
        assert_eq!(n, [(Put, 1)], "{:?}", b.kind);
        w.put(&record("backups", 2), &mut buf).unwrap();
        let mut read = vec![0u8; 4096];
        let got = SnapshotReader::new(std::sync::Arc::clone(&b.store), BASE)
            .repository("backups", &mut read)
            .unwrap()
            .expect("registered");
        assert_eq!(got, record("backups", 2), "{:?} replaced", b.kind);
    }
}

#[test]
fn a_missing_repository_costs_one_head_and_an_empty_one_four_requests() {
    for b in all() {
        let w = writer(&b);
        let mut buf = vec![0u8; 4096];
        let (out, n) = counted(b.meter(), || w.remove("nope", &mut buf).unwrap());
        assert_eq!(
            (out, n),
            (Removal::Missing, vec![(Head, 1)]),
            "{:?}",
            b.kind
        );
        w.put(&record("backups", 1), &mut buf).unwrap();
        let (out, n) = counted(b.meter(), || w.remove("backups", &mut buf).unwrap());
        assert_eq!(out, Removal::Removed, "{:?}", b.kind);
        assert_eq!(n, [(Head, 1), (List, 2), (Delete, 1)], "{:?}", b.kind);
        assert!(!registered(&b, "backups"));
    }
}

#[test]
fn a_held_snapshot_keeps_its_repository_and_a_released_one_does_not() {
    for b in all() {
        let w = writer(&b);
        let mut buf = vec![0u8; 1 << 16];
        w.put(&record("backups", 1), &mut buf).unwrap();
        let (marker, value) = held("backups", "daily-1");
        put(&b, &format!("snapshots/{marker}"), b"m");
        register(&b, "backups", "daily-1", &value);
        assert_eq!(
            w.remove("backups", &mut buf).unwrap(),
            Removal::InUse,
            "{:?}",
            b.kind
        );
        assert!(registered(&b, "backups"), "{:?} kept", b.kind);
        // Released: the register holds the released value.
        let key = snapshot_state_key(BASE, "backups", "daily-1").unwrap();
        let read = b.store.read_register(&key, &mut buf).unwrap().unwrap();
        assert!(
            b.store
                .swap_register(&key, read.generation, br#"{"_released":true}"#)
                .is_ok()
        );
        assert_eq!(
            w.remove("backups", &mut buf).unwrap(),
            Removal::Removed,
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_legacy_marker_and_another_repositorys_marker_are_judged_by_their_record() {
    for b in all() {
        let w = writer(&b);
        let mut buf = vec![0u8; 1 << 16];
        w.put(&record("backups", 1), &mut buf).unwrap();
        w.put(&record("archive", 1), &mut buf).unwrap();
        put(
            &b,
            "snapshots/backups#daily-1",
            &fixture("snapshot-finished.json"),
        );
        // A record that names another repository does not hold `archive`.
        put(
            &b,
            "snapshots/archive#daily-1",
            &fixture("snapshot-finished.json"),
        );
        assert_eq!(
            w.remove("archive", &mut buf).unwrap(),
            Removal::Removed,
            "{:?}",
            b.kind
        );
        assert_eq!(
            w.remove("backups", &mut buf).unwrap(),
            Removal::InUse,
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn an_unreadable_marker_does_not_hold_a_repository_and_a_flood_of_markers_is_refused() {
    let b = all().into_iter().next().unwrap();
    let w = writer(&b);
    let mut buf = vec![0u8; 4096];
    w.put(&record("backups", 1), &mut buf).unwrap();
    put(&b, "snapshots/backups#junk", b"not json");
    assert_eq!(w.remove("backups", &mut buf).unwrap(), Removal::Removed);
    w.put(&record("backups", 1), &mut buf).unwrap();
    for i in 0..=MAX_SNAPSHOT_MARKERS {
        put(&b, &format!("snapshots/backups#junk-{i:05}"), b"x");
    }
    assert!(matches!(
        w.remove("backups", &mut buf),
        Err(RepositoryWriteError::TooManyMarkers)
    ));
    assert!(registered(&b, "backups"));
}
