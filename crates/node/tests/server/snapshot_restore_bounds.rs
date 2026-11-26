//! docs/specs/snapshot-restore AC-2 and AC-4: a restore is held to its
//! memory, a snapshot still being taken is 409, and a request over the
//! shard bound is 400, each before any write.

// The records here are built to be read, a few hundred bytes to some hundred KiB.
#![allow(clippy::disallowed_methods)]

use oss_blob::{BlobStore, S3Op};
use oss_format::{
    IndexCapture, MAX_SNAPSHOT_RECORD_BYTES, ShardCapture, ShardRef, SnapshotDraft, SnapshotState,
};

use crate::snapshot_restore::{
    RESTORE, Rig, live_index, read, requests, restored_uuid, rig_sized, send,
};
use crate::snapshot_take::TAKE;

const POOL_SLACK: usize = 2 << 20;

/// A rig whose pool holds the record's reservation and `slack` more, with
/// the repository registered and `bytes` taken from `logs`.
fn taken_in(slack: usize, chunk: usize, bytes: &[u8]) -> Rig {
    let r = rig_sized(
        true,
        "",
        MAX_SNAPSHOT_RECORD_BYTES.saturating_add(slack),
        chunk,
    );
    let repo =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some(repo)).status,
        200
    );
    live_index(&r, "logs", "u1", 2, bytes);
    let made = send(
        &r,
        "PUT",
        "/_snapshot/backups/daily?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    r
}

fn writes(r: &Rig) -> u64 {
    [S3Op::Put, S3Op::Copy]
        .iter()
        .map(|op| r.store.meter().count(*op))
        .sum()
}

// AC-4: the buffers are reserved before anything is written.
#[test]
fn a_restore_the_body_pool_cannot_hold_is_429_and_writes_nothing() {
    let r = taken_in(POOL_SLACK, 4 << 20, b"payload");
    let (before, written) = (requests(&r), writes(&r));
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    let got = send(&r, "POST", RESTORE, Some(rename));
    assert_eq!(got.status, 429, "{}", got.text());
    assert!(requests(&r) > before, "the snapshot was read");
    assert_eq!(writes(&r), written, "and nothing was written");
    let mut buf = vec![0u8; 1 << 16];
    assert!(r.descriptors.get("copy", &mut buf).unwrap().is_none());
}

// AC-4: a file far larger than the memory the pool leaves is copied, so the
// copy's memory is two chunks and not the file.
#[test]
fn a_file_larger_than_the_pool_leaves_is_copied_through_two_chunks() {
    let file = vec![7u8; 6 << 20];
    let r = taken_in(POOL_SLACK, 64 << 10, &file);
    let rename = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    let got = send(&r, "POST", RESTORE, Some(rename));
    assert_eq!(got.status, 200, "{}", got.text());
    let mut buf = vec![0u8; 1 << 16];
    assert!(r.descriptors.get("copy", &mut buf).unwrap().is_some());
    let uuid = restored_uuid(&r, "copy");
    for shard in 0..2 {
        let at = ShardRef {
            index: "copy",
            uuid: &uuid,
            shard,
        };
        let blob = format!("{}t=1/_0.si", at.base_path("").unwrap());
        assert!(
            read(&r, &blob) == file,
            "shard {shard} holds the 6 MiB, byte for byte"
        );
    }
}

/// A held register of `draft` for `snapshot` of the repository `backups`.
fn hold(r: &Rig, snapshot: &str, draft: &SnapshotDraft, finished: bool) {
    let mut value = vec![0u8; 8 << 20];
    let n = SnapshotState::write_held_draft("marker", draft, finished, &mut value).unwrap();
    r.store
        .create_register(
            &oss_format::snapshot_state_key("", "backups", snapshot).unwrap(),
            &value[..n],
        )
        .unwrap();
}

fn draft(name: &str, shards: u32, captured: u32) -> SnapshotDraft {
    SnapshotDraft {
        repo: "backups".into(),
        name: name.into(),
        uuid: "snapuuid00000000000009".into(),
        storage_generation: None,
        shallow: true,
        start_millis: 1,
        end_millis: 2,
        indices: vec![IndexCapture {
            name: "big".into(),
            uuid: "uuidbig0000000000000A".into(),
            number_of_shards: shards,
            mapping: None,
            settings: None,
            shards: (0..captured)
                .map(|shard| ShardCapture {
                    shard,
                    term: 1,
                    files: vec![("_0.si".into(), "t=1/_0.si".into())],
                    lengths: vec![("_0.si".into(), 1)],
                })
                .collect(),
            aliases: vec![],
            aliases_truncated: false,
        }],
    }
}

fn repository(r: &Rig) {
    let repo =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(send(r, "PUT", "/_snapshot/backups", Some(repo)).status, 200);
}

// AC-2: a snapshot still being taken.
#[test]
fn a_snapshot_still_being_taken_is_409_and_writes_nothing() {
    let r = rig_sized(true, "", 64 << 20, 64);
    repository(&r);
    hold(&r, "busy", &draft("busy", 1, 0), false);
    let written = writes(&r);
    let got = send(&r, "POST", "/_snapshot/backups/busy/_restore", Some("{}"));
    assert_eq!(got.status, 409, "{}", got.text());
    assert!(
        got.text().contains("snapshot_in_progress"),
        "{}",
        got.text()
    );
    assert_eq!(writes(&r), written);
}

// AC-2: the shard bound, at the route.
#[test]
fn a_restore_over_4096_shards_is_400_and_writes_nothing() {
    let r = rig_sized(true, "", 64 << 20, 64);
    repository(&r);
    hold(&r, "wide", &draft("wide", 4097, 4097), true);
    let written = writes(&r);
    let got = send(&r, "POST", "/_snapshot/backups/wide/_restore", Some("{}"));
    assert_eq!(got.status, 400, "{}", got.text());
    assert!(got.text().contains("snapshot_too_large"), "{}", got.text());
    assert_eq!(writes(&r), written);
    // The bound is exact: 4,096 shards pass it and fail later, for want of files.
    hold(&r, "fits", &draft("fits", 4096, 4096), true);
    let got = send(&r, "POST", "/_snapshot/backups/fits/_restore", Some("{}"));
    assert!(!got.text().contains("snapshot_too_large"), "{}", got.text());
}
