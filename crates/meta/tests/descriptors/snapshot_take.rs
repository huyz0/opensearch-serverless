//! docs/specs/snapshot-take AC-2, AC-4: reserving a snapshot name, writing
//! its protective marker, finishing it and cleaning up after a failure, per
//! backend, with request counts.

use std::sync::Arc;

use oss_blob::SliceSource;
use oss_format::{IndexCapture, ShardCapture, SnapshotDraft, SnapshotState, snapshot_state_key};
use oss_meta::{SnapshotReader, SnapshotTaker, TakeError};

use crate::{BASE, Backend, Kind, all, counted};

/// The requests a backend made: exact on the object stores; a file store
/// swaps a register with an extra read, so it makes at least as many.
fn requests(b: &Backend, n: &[(oss_blob::S3Op, u64)], expected: u64) {
    let total: u64 = n.iter().map(|(_, c)| c).sum();
    if b.kind == Kind::Fs {
        assert!(total >= expected, "{:?} {n:?}", b.kind);
    } else {
        assert_eq!(total, expected, "{:?} {n:?}", b.kind);
    }
}

const GEN_A: &str = "genAAAAAAAAAAAAAAAAAAA";
const GEN_B: &str = "genBBBBBBBBBBBBBBBBBBB";

fn taker(b: &Backend) -> SnapshotTaker {
    SnapshotTaker::new(Arc::clone(&b.store), BASE)
}

fn draft(name: &str, generation: &str, captured: bool) -> SnapshotDraft {
    SnapshotDraft {
        repo: "backups".into(),
        name: name.into(),
        uuid: format!("snap-{generation}"),
        storage_generation: Some(generation.into()),
        shallow: true,
        start_millis: 1_000,
        end_millis: if captured { 2_000 } else { 1_000 },
        indices: vec![IndexCapture {
            name: "logs".into(),
            uuid: "uuidlogs".into(),
            number_of_shards: 1,
            mapping: captured.then(|| "{}".to_owned()),
            settings: None,
            shards: if captured {
                vec![ShardCapture {
                    shard: 0,
                    term: 1,
                    files: vec![("segments_1".into(), "t=1/a.seg#0:10".into())],
                    lengths: vec![("segments_1".into(), 10)],
                }]
            } else {
                vec![]
            },
            failed_shards: vec![],
            aliases: vec![],
            aliases_truncated: false,
        }],
    }
}

fn read(b: &Backend, name: &str) -> Option<(u32, u32)> {
    let mut buf = vec![0u8; 1 << 16];
    SnapshotReader::new(Arc::clone(&b.store), BASE)
        .snapshot("backups", name, &mut buf)
        .unwrap()
        .map(|r| {
            let i = &r.summary.indices[0];
            (i.shards, i.captured)
        })
}

fn marker_exists(b: &Backend, marker: &str) -> bool {
    b.store
        .head(&format!("{BASE}snapshots/{marker}"))
        .unwrap()
        .is_some()
}

fn release(b: &Backend, name: &str) {
    let key = snapshot_state_key(BASE, "backups", name).unwrap();
    let mut buf = vec![0u8; 1 << 16];
    let at = b.store.read_register(&key, &mut buf).unwrap().unwrap();
    b.store
        .swap_register(&key, at.generation, SnapshotState::released_value())
        .unwrap();
}

#[test]
fn a_take_reserves_marks_and_finishes_and_reads_as_captured() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        let (r, n) = counted(b.meter(), || {
            let r = t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
            t.publish_marker(&r, &draft("daily", GEN_A, false), &mut v)
                .unwrap();
            t.finish(&r, &draft("daily", GEN_A, true), &mut v).unwrap();
            r
        });
        // Register read, legacy head, create; marker put, confirm read;
        // finish read, swap.
        requests(&b, &n, 7);
        assert_eq!(read(&b, "daily"), Some((1, 1)), "{:?}", b.kind);
        assert!(marker_exists(&b, r.marker()), "{:?}", b.kind);
    }
}

#[test]
fn a_held_name_is_refused_and_a_released_one_is_taken_again() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
        assert!(
            matches!(
                t.reserve(&draft("daily", GEN_B, false), &mut v),
                Err(TakeError::AlreadyExists)
            ),
            "{:?}",
            b.kind
        );
        release(&b, "daily");
        assert_eq!(read(&b, "daily"), None, "{:?}", b.kind);
        let (_, n) = counted(b.meter(), || {
            t.reserve(&draft("daily", GEN_B, false), &mut v).unwrap()
        });
        // Released: the read and the swap, no legacy check.
        requests(&b, &n, 2);
        assert_eq!(read(&b, "daily"), Some((1, 0)), "{:?} in progress", b.kind);
    }
}

#[test]
fn a_legacy_record_and_a_long_name_hold_the_name() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        b.store
            .put(
                &format!("{BASE}snapshots/backups#old"),
                &mut SliceSource::new(b"{}"),
                &mut [0u8; 64],
            )
            .unwrap();
        assert!(
            matches!(
                t.reserve(&draft("old", GEN_A, false), &mut v),
                Err(TakeError::AlreadyExists)
            ),
            "{:?}",
            b.kind
        );
        let long = "n".repeat(255 - "backups#".len());
        let (_, n) = counted(b.meter(), || t.reserve(&draft(&long, GEN_A, false), &mut v));
        assert!(
            n.iter().all(|(op, _)| *op != oss_blob::S3Op::List),
            "{:?}",
            b.kind
        );
        let too_long = format!("{long}n");
        let (out, n) = counted(b.meter(), || {
            t.reserve(&draft(&too_long, GEN_A, false), &mut v)
        });
        assert!(matches!(out, Err(TakeError::KeyTooLong)), "{:?}", b.kind);
        assert!(n.is_empty(), "{:?} {n:?}", b.kind);
    }
}

#[test]
fn an_abort_releases_the_name_and_removes_the_marker() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        let r = t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
        t.publish_marker(&r, &draft("daily", GEN_A, false), &mut v)
            .unwrap();
        assert!(marker_exists(&b, r.marker()), "{:?}", b.kind);
        t.abort(&r, &mut v).unwrap();
        assert_eq!(read(&b, "daily"), None, "{:?}", b.kind);
        assert!(!marker_exists(&b, r.marker()), "{:?}", b.kind);
        // The name is free again.
        t.reserve(&draft("daily", GEN_B, false), &mut v).unwrap();
    }
}

#[test]
fn an_abort_never_touches_a_later_take_of_the_same_name() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        let a = t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
        t.publish_marker(&a, &draft("daily", GEN_A, false), &mut v)
            .unwrap();
        // A delete and a re-take of the name while the first take fails.
        release(&b, "daily");
        let second = t.reserve(&draft("daily", GEN_B, false), &mut v).unwrap();
        t.publish_marker(&second, &draft("daily", GEN_B, false), &mut v)
            .unwrap();
        t.abort(&a, &mut v).unwrap();
        assert!(!marker_exists(&b, a.marker()), "{:?} own marker", b.kind);
        assert!(
            marker_exists(&b, second.marker()),
            "{:?} other's marker",
            b.kind
        );
        assert_eq!(read(&b, "daily"), Some((1, 0)), "{:?} still held", b.kind);
        t.finish(&second, &draft("daily", GEN_B, true), &mut v)
            .unwrap();
        assert_eq!(read(&b, "daily"), Some((1, 1)), "{:?}", b.kind);
    }
}

#[test]
fn an_abort_after_the_snapshot_finished_leaves_it_standing() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        let r = t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
        t.publish_marker(&r, &draft("daily", GEN_A, false), &mut v)
            .unwrap();
        t.finish(&r, &draft("daily", GEN_A, true), &mut v).unwrap();
        t.abort(&r, &mut v).unwrap();
        assert_eq!(read(&b, "daily"), Some((1, 1)), "{:?}", b.kind);
        assert!(marker_exists(&b, r.marker()), "{:?}", b.kind);
    }
}

#[test]
fn a_take_that_was_overtaken_cannot_publish_or_finish() {
    for b in all() {
        let t = taker(&b);
        let mut v = vec![0u8; 1 << 16];
        let a = t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
        release(&b, "daily");
        let err = t
            .publish_marker(&a, &draft("daily", GEN_A, false), &mut v)
            .unwrap_err();
        assert!(matches!(err, TakeError::Conflict(_)), "{:?} {err}", b.kind);
        let err = t
            .finish(&a, &draft("daily", GEN_A, true), &mut v)
            .unwrap_err();
        assert!(matches!(err, TakeError::Conflict(_)), "{:?} {err}", b.kind);
        // A finished snapshot cannot be finished again.
        let c = t.reserve(&draft("daily", GEN_B, false), &mut v).unwrap();
        t.finish(&c, &draft("daily", GEN_B, true), &mut v).unwrap();
        let err = t
            .finish(&c, &draft("daily", GEN_B, true), &mut v)
            .unwrap_err();
        assert!(matches!(err, TakeError::Conflict(_)), "{:?} {err}", b.kind);
        // A marker created twice is a conflict.
        let d = t.reserve(&draft("other", GEN_A, false), &mut v).unwrap();
        t.publish_marker(&d, &draft("other", GEN_A, false), &mut v)
            .unwrap();
        assert!(
            matches!(
                t.publish_marker(&d, &draft("other", GEN_A, false), &mut v),
                Err(TakeError::Conflict(_))
            ),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_record_that_does_not_fit_is_reported_with_the_length_it_needs() {
    for b in all() {
        let t = taker(&b);
        let mut small = vec![0u8; 64];
        let err = t
            .reserve(&draft("daily", GEN_A, false), &mut small)
            .unwrap_err();
        assert!(
            matches!(err, TakeError::RecordTooLarge(n) if n > 64),
            "{:?} {err}",
            b.kind
        );
        let mut v = vec![0u8; 1 << 16];
        let r = t.reserve(&draft("daily", GEN_A, false), &mut v).unwrap();
        let err = t
            .finish(&r, &draft("daily", GEN_A, true), &mut small)
            .unwrap_err();
        assert!(
            matches!(err, TakeError::RecordTooLarge(_)),
            "{:?} {err}",
            b.kind
        );
    }
}
