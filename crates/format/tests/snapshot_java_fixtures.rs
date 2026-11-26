//! docs/specs/snapshot-read: the records the Java shell wrote are read.

// ARITH: test sizes are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{
    AliasCapture, IndexCapture, RepositoryRecord, ShardCapture, SnapshotDraft, SnapshotState,
    SnapshotSummary,
};

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

#[test]
fn a_record_and_register_written_here_are_the_bytes_the_java_wrote() {
    let draft = SnapshotDraft {
        repo: "backups".into(),
        name: "daily-1".into(),
        uuid: "snapuuid00000000000001".into(),
        storage_generation: Some("gen0000000000000000001".into()),
        shallow: false,
        start_millis: 1_700_000_001_000,
        end_millis: 1_700_000_002_000,
        global_state: None,
        indices: vec![IndexCapture {
            name: "logs".into(),
            uuid: "uuidlogs0000000000000A".into(),
            number_of_shards: 1,
            mapping: Some(r#"{"properties":{"n":{"type":"long"}}}"#.into()),
            settings: None,
            shards: vec![ShardCapture {
                shard: 0,
                term: 2,
                files: vec![
                    ("segments_1".into(), "t=2/_0-abc.seg#110:50".into()),
                    ("_0.cfs".into(), "t=2/_0-abc.seg#10:100".into()),
                ],
                lengths: vec![("segments_1".into(), 50), ("_0.cfs".into(), 100)],
            }],
            failed_shards: vec![],
            aliases: vec![],
            aliases_truncated: false,
        }],
    };
    let java = fixture("snapshot-finished.json");
    let mut buf = vec![0u8; java.len()];
    let n = draft.write(&mut buf).expect("writes");
    assert_eq!(
        String::from_utf8_lossy(&buf[..n]),
        String::from_utf8_lossy(&java)
    );

    let state = fixture("state-finished.bin");
    let SnapshotState::Held { marker, .. } = SnapshotState::parse(&state).expect("parses") else {
        panic!("held");
    };
    let mut out = vec![0u8; state.len()];
    let n = SnapshotState::write_held(marker, &java, true, &mut out).expect("writes");
    assert_eq!(out[..n], state[..]);
    assert!(SnapshotState::write_held(marker, &java, true, &mut out[..n - 1]).is_err());
}

// The record the Java shell writes for an index in two aliases and cut at
// the bound: read here as the Java wrote it, and written here byte for byte.
#[test]
fn the_aliases_the_java_wrote_are_read_and_written_back_identically() {
    let java = fixture("record-with-aliases.json");
    let got = oss_format::RestoreIndex::find(&java, "index")
        .expect("reads")
        .expect("named");
    assert_eq!(
        got.aliases,
        [
            AliasCapture {
                name: "logs-read".into(),
                write: false
            },
            AliasCapture {
                name: "logs-write".into(),
                write: true
            }
        ]
    );
    assert!(got.aliases_truncated);
    let draft = SnapshotDraft {
        repo: "repo".into(),
        name: "view".into(),
        uuid: "uuid".into(),
        storage_generation: None,
        shallow: false,
        start_millis: 1000,
        end_millis: 2000,
        global_state: None,
        indices: vec![IndexCapture {
            name: "index".into(),
            uuid: "uuid-index".into(),
            number_of_shards: 1,
            mapping: None,
            settings: None,
            shards: vec![ShardCapture {
                shard: 0,
                term: 1,
                files: vec![("file".into(), "t=1".into())],
                lengths: vec![],
            }],
            failed_shards: vec![],
            aliases: got.aliases.clone(),
            aliases_truncated: true,
        }],
    };
    let mut buf = vec![0u8; java.len() + 16];
    let n = draft.write(&mut buf).expect("writes");
    assert_eq!(
        String::from_utf8_lossy(&buf[..n]),
        String::from_utf8_lossy(&java)
    );
}
