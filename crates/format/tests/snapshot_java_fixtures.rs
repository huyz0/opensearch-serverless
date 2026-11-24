//! docs/specs/snapshot-read: the records the Java shell wrote are read.

// ARITH: test sizes are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{RepositoryRecord, SnapshotState, SnapshotSummary};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_java_repository_record_is_read() {
    let r = RepositoryRecord::parse(&fixture("repository.json")).expect("parses");
    assert_eq!((r.name.as_str(), r.repo_type.as_str()), ("backups", "fs"));
    assert_eq!(r.created_at_millis, 1_700_000_000_000);
    let get = |k: &str| {
        r.settings
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("location"), Some("backups"));
    assert_eq!(get("max_snapshot_bytes_per_sec"), Some("5"));
    assert!(r.shallow_by_default());
}

#[test]
fn a_finished_java_snapshot_reads_to_its_summary() {
    let s = SnapshotSummary::parse(&fixture("snapshot-finished.json")).expect("parses");
    assert_eq!((s.repo.as_str(), s.name.as_str()), ("backups", "daily-1"));
    assert_eq!(s.uuid.as_deref(), Some("snapuuid00000000000001"));
    assert_eq!(
        s.storage_generation.as_deref(),
        Some("gen0000000000000000001")
    );
    assert!(!s.shallow);
    assert_eq!(
        (s.start_millis, s.end_millis),
        (1_700_000_001_000, 1_700_000_002_000)
    );
    assert_eq!(s.indices.len(), 1);
    let logs = &s.indices[0];
    assert_eq!(
        (logs.name.as_str(), logs.shards, logs.captured),
        ("logs", 1, 1)
    );
    assert!(!s.in_progress());
}

#[test]
fn an_unfinished_java_snapshot_is_in_progress_by_its_record() {
    let s = SnapshotSummary::parse(&fixture("snapshot-in-progress.json")).expect("parses");
    assert!(s.shallow && s.in_progress());
    let metrics = s
        .indices
        .iter()
        .find(|i| i.name == "metrics")
        .expect("metrics");
    assert_eq!((metrics.shards, metrics.captured), (3, 0));
    assert!(metrics.capturing());
}

#[test]
fn the_java_state_register_values_hold_their_record() {
    for (file, finished, name) in [
        ("state-finished.bin", true, "daily-1"),
        ("state-in-progress.bin", false, "daily-2"),
    ] {
        let bytes = fixture(file);
        let SnapshotState::Held {
            marker,
            record,
            finished: flag,
        } = SnapshotState::parse(&bytes).expect("parses")
        else {
            panic!("held");
        };
        assert!(marker.starts_with("v2-marker#hash#gen"), "{marker}");
        assert_eq!(flag, finished);
        assert_eq!(SnapshotSummary::parse(record).expect("record").name, name);
    }
}
