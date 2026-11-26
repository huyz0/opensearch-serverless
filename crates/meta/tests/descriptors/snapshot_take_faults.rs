//! docs/specs/snapshot-take AC-4: a take that fails at any store call, and
//! two takes of one name at once.

use std::sync::{Arc, Barrier};

use oss_blob::BlobStore;
use oss_format::{IndexCapture, ShardCapture, SnapshotDraft};
use oss_meta::{SnapshotReader, SnapshotTaker, TakeError};

use crate::probe::{Quirk, Quirky};
use crate::{BASE, all};

pub(crate) fn draft(name: &str, generation: &str, captured: bool) -> SnapshotDraft {
    SnapshotDraft {
        repo: "backups".into(),
        name: name.into(),
        uuid: format!("snap-{generation}"),
        storage_generation: Some(generation.into()),
        shallow: true,
        start_millis: 1,
        end_millis: 2,
        indices: vec![IndexCapture {
            name: "logs".into(),
            uuid: "u1".into(),
            number_of_shards: 1,
            mapping: None,
            settings: None,
            shards: if captured {
                vec![ShardCapture {
                    shard: 0,
                    term: 1,
                    files: vec![("segments_1".into(), "t=1".into())],
                    lengths: vec![("segments_1".into(), 7)],
                }]
            } else {
                vec![]
            },
            aliases: vec![],
            aliases_truncated: false,
        }],
    }
}

/// A whole take: reserve, mark, finish. When one step fails the take is
/// aborted, as the route does; the error is the first failure's.
pub(crate) fn take(t: &SnapshotTaker, name: &str, generation: &str) -> Result<(), TakeError> {
    let mut v = vec![0u8; 1 << 16];
    let r = t.reserve(&draft(name, generation, false), &mut v)?;
    let run = |v: &mut [u8]| {
        t.publish_marker(&r, &draft(name, generation, false), v)?;
        t.finish(&r, &draft(name, generation, true), v)
    };
    run(&mut v).inspect_err(|_| {
        let _ = t.abort(&r, &mut v);
    })
}

pub(crate) fn markers(store: &dyn BlobStore) -> usize {
    let mut n = 0;
    store
        // LIST: a test counting what a failed take left.
        .list_prefix(&format!("{BASE}snapshots/"), None, 100, &mut |_, _| n += 1)
        .unwrap();
    n
}

pub(crate) fn read(store: &Arc<dyn BlobStore>) -> Option<u32> {
    let mut buf = vec![0u8; 1 << 16];
    SnapshotReader::new(Arc::clone(store), BASE)
        .snapshot("backups", "daily", &mut buf)
        .unwrap()
        .map(|r| r.summary.indices[0].captured)
}

#[test]
fn a_take_that_fails_at_any_store_call_leaves_nothing_that_names_storage() {
    // reserve 3 requests, marker 2, finish 2.
    for n in 1..=8 {
        let quirky = Arc::new(Quirky::new(Quirk::FailRequest(n)));
        let store: Arc<dyn BlobStore> = Arc::clone(&quirky) as Arc<dyn BlobStore>;
        let t = SnapshotTaker::new(Arc::clone(&store), BASE);
        let out = take(&t, "daily", "genAAAAAAAAAAAAAAAAAAA");
        let inner = &quirky.inner;
        if n <= 7 {
            assert!(out.is_err(), "request {n} failed, so the take did");
            assert_eq!(read(inner), None, "request {n}: the name is free");
            // The take's own marker is gone; the failing request may be the
            // marker's own create, which then never existed.
            assert_eq!(markers(&**inner), 0, "request {n}: no marker left");
            // A later take of the name succeeds.
            let again = SnapshotTaker::new(Arc::clone(inner), BASE);
            take(&again, "daily", "genBBBBBBBBBBBBBBBBBBB").unwrap();
            assert_eq!(read(inner), Some(1), "request {n}: retaken");
        } else {
            out.unwrap();
            assert_eq!(read(inner), Some(1), "the take completed");
            assert_eq!(markers(&**inner), 1);
        }
    }
}

#[test]
fn two_takes_of_one_name_at_once_give_one_snapshot() {
    for b in all() {
        for round in 0..10 {
            let name = format!("daily{round}");
            let gate = Arc::new(Barrier::new(2));
            let results: Vec<Result<(), TakeError>> = std::thread::scope(|s| {
                ["genAAAAAAAAAAAAAAAAAAA", "genBBBBBBBBBBBBBBBBBBB"]
                    .map(|generation| {
                        let t = SnapshotTaker::new(Arc::clone(&b.store), BASE);
                        let gate = Arc::clone(&gate);
                        let name = name.clone();
                        s.spawn(move || {
                            gate.wait();
                            take(&t, &name, generation)
                        })
                    })
                    .into_iter()
                    .map(|h| h.join().unwrap())
                    .collect()
            });
            let won = results.iter().filter(|r| r.is_ok()).count();
            assert_eq!(won, 1, "{:?} round {round}: {results:?}", b.kind);
            assert!(
                results
                    .iter()
                    .any(|r| matches!(r, Err(TakeError::AlreadyExists | TakeError::Conflict(_)))),
                "{:?} {results:?}",
                b.kind
            );
            let mut buf = vec![0u8; 1 << 16];
            let read = SnapshotReader::new(Arc::clone(&b.store), BASE)
                .snapshot("backups", &name, &mut buf)
                .unwrap()
                .expect("one snapshot");
            assert_eq!(read.summary.indices[0].captured, 1, "{:?}", b.kind);
        }
    }
}
