//! docs/specs/point-in-time-store/SPEC.md: the point-in-time store, one suite
//! over every backend with exact request counts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::S3Op::{Delete, Get, GetRange, Head, List, Put};
use oss_blob::{BlobStore, SliceSource};
use oss_meta::{MetaError, PitFile, PitShard, PointInTime, PointInTimeStore};

use crate::{BASE, Backend, all, counted};

pub(crate) const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const OTHER: &str = "BBBBBBBBBBBBBBBBBBBBBB";
pub(crate) const BUF: usize = 1 << 16;

pub(crate) fn view(id: &str, expires: u64) -> PointInTime {
    PointInTime {
        id: id.to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("u-1".to_owned()),
        expires_at_millis: expires,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 3,
            files: vec![PitFile {
                name: "_0.si".to_owned(),
                location: "t=3".to_owned(),
                length: Some(7),
            }],
        }],
    }
}

pub(crate) fn capture(id: &str, expires: u64) -> PointInTime {
    PointInTime {
        capturing: true,
        shards: Vec::new(),
        ..view(id, expires)
    }
}

/// A clock a test moves.
fn clock() -> (Arc<AtomicU64>, oss_meta::PitClock) {
    let now = Arc::new(AtomicU64::new(1_000));
    let read = Arc::clone(&now);
    (now, Arc::new(move || read.load(Ordering::SeqCst)))
}

pub(crate) fn store_of(store: Arc<dyn BlobStore>) -> (Arc<AtomicU64>, PointInTimeStore) {
    let (now, c) = clock();
    (now, PointInTimeStore::new(store, BASE, c).unwrap())
}

fn pits(b: &Backend) -> (Arc<AtomicU64>, PointInTimeStore) {
    store_of(Arc::clone(&b.store))
}

pub(crate) fn buf() -> Vec<u8> {
    vec![0u8; BUF]
}

#[test]
fn a_created_view_reads_back_and_the_id_is_taken() {
    for b in all() {
        let (_, p) = pits(&b);
        let (done, sent) = counted(b.meter(), || p.create(&view(ID, 5_000), &mut buf()));
        done.unwrap();
        assert_eq!(
            sent,
            b.budget(&[(Head, 1), (Put, 2)], &[(Head, 1), (Put, 2)]),
            "{:?}",
            b.kind
        );
        let (got, sent) = counted(b.meter(), || p.read(ID, &mut buf()).unwrap());
        assert_eq!(got, Some(view(ID, 5_000)));
        assert_eq!(sent, [(Get, 1)], "one register read: {:?}", b.kind);
        assert!(matches!(
            p.create(&view(ID, 9_000), &mut buf()),
            Err(MetaError::AlreadyExists)
        ));
        assert_eq!(p.read("CCCCCCCCCCCCCCCCCCCCCC", &mut buf()).unwrap(), None);
        assert!(
            p.create(&view("bad id", 1), &mut buf()).is_err(),
            "an id is 22 URL-safe characters"
        );
    }
}

#[test]
fn a_capture_is_not_readable_until_it_is_finished_under_the_same_id() {
    for b in all() {
        let (_, p) = pits(&b);
        p.create(&capture(ID, 5_000), &mut buf()).unwrap();
        assert_eq!(p.read(ID, &mut buf()).unwrap(), None, "a marker is no view");
        let finished = view(ID, 1);
        p.finish(&finished, &mut buf()).unwrap();
        let got = p.read(ID, &mut buf()).unwrap().expect("live");
        assert_eq!(got.shards, finished.shards);
        assert_eq!(
            got.expires_at_millis, 5_000,
            "the capture's deadline stands"
        );
        assert!(
            p.finish(&finished, &mut buf()).is_err(),
            "no longer capturing"
        );
    }
}

#[test]
fn finishing_refuses_a_capture_that_is_gone_another_index_or_itself_a_capture() {
    for b in all() {
        let (_, p) = pits(&b);
        assert!(p.finish(&view(ID, 1), &mut buf()).is_err(), "never begun");
        p.create(&capture(ID, 5_000), &mut buf()).unwrap();
        let mut other = view(ID, 1);
        other.index = "other".to_owned();
        assert!(p.finish(&other, &mut buf()).is_err(), "another index");
        let mut other = view(ID, 1);
        other.index_uuid = Some("u-2".to_owned());
        assert!(p.finish(&other, &mut buf()).is_err(), "another incarnation");
        assert!(
            p.finish(&capture(ID, 1), &mut buf()).is_err(),
            "a capture does not finish one"
        );
        assert!(p.release(ID, &mut buf()).unwrap());
        assert!(p.finish(&view(ID, 1), &mut buf()).is_err(), "released");
    }
}

#[test]
fn a_keep_alive_only_moves_the_deadline_later() {
    for b in all() {
        let (now, p) = pits(&b);
        p.create(&view(ID, 5_000), &mut buf()).unwrap();
        assert!(
            p.extend(ID, 4_000, None, &mut buf()).unwrap(),
            "live, already holds it"
        );
        assert_eq!(
            p.read(ID, &mut buf()).unwrap().unwrap().expires_at_millis,
            5_000
        );
        assert!(p.extend(ID, 8_000, None, &mut buf()).unwrap());
        let read = p.read(ID, &mut buf()).unwrap().unwrap();
        assert_eq!(read.expires_at_millis, 8_000);
        assert_eq!(read.index_uuid.as_deref(), Some("u-1"));
        assert!(
            !p.extend(ID, 9_000, Some(5_000), &mut buf()).unwrap(),
            "the deadline moved from what was seen"
        );
        assert!(p.extend(ID, 9_000, Some(8_000), &mut buf()).unwrap());
        assert!(
            !p.extend(OTHER, 9_000, None, &mut buf()).unwrap(),
            "no such view"
        );
        now.store(9_000, Ordering::SeqCst);
        assert!(
            !p.extend(ID, 20_000, None, &mut buf()).unwrap(),
            "expired views are not extended"
        );
    }
}

#[test]
fn a_capture_is_not_extended() {
    for b in all() {
        let (_, p) = pits(&b);
        p.create(&capture(ID, 5_000), &mut buf()).unwrap();
        assert!(!p.extend(ID, 8_000, None, &mut buf()).unwrap());
    }
}

#[test]
fn a_released_id_cannot_be_created_read_or_released_again() {
    for b in all() {
        let (_, p) = pits(&b);
        p.create(&view(ID, 5_000), &mut buf()).unwrap();
        let (released, sent) = counted(b.meter(), || p.release(ID, &mut buf()).unwrap());
        assert!(released);
        assert_eq!(
            sent,
            b.budget(
                &[(Get, 1), (Put, 1), (Delete, 1)],
                &[(Get, 1), (GetRange, 1), (Put, 1), (Delete, 1)]
            ),
            "{:?}",
            b.kind
        );
        assert!(!p.release(ID, &mut buf()).unwrap());
        assert_eq!(p.read(ID, &mut buf()).unwrap(), None);
        assert!(matches!(
            p.create(&view(ID, 9_000), &mut buf()),
            Err(MetaError::AlreadyExists)
        ));
        assert!(!p.extend(ID, 9_000, None, &mut buf()).unwrap());
        assert!(
            !b.store.head(&format!("{BASE}pits/{ID}")).unwrap().is_some(),
            "the marker is gone"
        );
    }
}

#[test]
fn live_views_are_listed_and_expired_ones_are_claimed_and_removed() {
    for b in all() {
        let (now, p) = pits(&b);
        p.create(&view(ID, 5_000), &mut buf()).unwrap();
        p.create(&view(OTHER, 2_000), &mut buf()).unwrap();
        // Something in the inventory that is no id is left alone.
        b.store
            .put(
                &format!("{BASE}pits/note"),
                &mut SliceSource::new(b"x"),
                &mut [0u8; 16],
            )
            .unwrap();
        let ((live, _), sent) = counted(b.meter(), || (p.live(&mut buf()).unwrap(), ()));
        assert_eq!(live.len(), 2);
        assert!(sent.contains(&(List, 1)), "{:?}", b.kind);
        now.store(3_000, Ordering::SeqCst);
        let live = p.live(&mut buf()).unwrap();
        assert_eq!(live.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), [ID]);
        assert_eq!(
            p.read(OTHER, &mut buf()).unwrap(),
            None,
            "expiry released it"
        );
        assert!(
            b.store
                .head(&format!("{BASE}pits/{OTHER}"))
                .unwrap()
                .is_none()
        );
        now.store(6_000, Ordering::SeqCst);
        assert_eq!(p.reap(&mut buf()).unwrap(), 1);
        assert_eq!(p.reap(&mut buf()).unwrap(), 0);
        assert!(p.live(&mut buf()).unwrap().is_empty());
    }
}

#[test]
fn a_view_that_cannot_be_read_stands_in_as_a_placeholder() {
    for b in all() {
        let (_, p) = pits(&b);
        b.store
            .create_register(&format!("{BASE}pit-states/{ID}"), b"not json")
            .unwrap();
        b.store
            .put(
                &format!("{BASE}pits/{ID}"),
                &mut SliceSource::new(b"x"),
                &mut [0u8; 16],
            )
            .unwrap();
        let live = p.live(&mut buf()).unwrap();
        assert_eq!(live.len(), 1);
        let held = live.first().unwrap();
        assert_eq!((held.id.as_str(), held.index.as_str()), (ID, ""));
        assert!(held.is_placeholder() && !held.capturing);
        assert_eq!(held.expires_at_millis, i64::MAX as u64);
        assert_eq!(p.reap(&mut buf()).unwrap(), 0, "left alone");
    }
}

#[test]
fn a_view_written_before_the_state_register_is_adopted_by_a_read() {
    for b in all() {
        let (_, p) = pits(&b);
        let mut v = buf();
        let n = view(ID, 5_000).write(&mut v).unwrap();
        b.store
            .put(
                &format!("{BASE}pits/{ID}"),
                &mut SliceSource::new(&v[..n]),
                &mut [0u8; 4096],
            )
            .unwrap();
        let (got, sent) = counted(b.meter(), || p.read(ID, &mut buf()).unwrap());
        assert_eq!(got, Some(view(ID, 5_000)));
        assert_eq!(
            sent.iter().find(|(op, _)| *op == Put).map(|x| x.1),
            Some(1),
            "the register is created: {:?}",
            b.kind
        );
        let (again, sent) = counted(b.meter(), || p.read(ID, &mut buf()).unwrap());
        assert_eq!(again, got);
        assert_eq!(sent, [(Get, 1)], "adopted once");
        assert!(matches!(
            p.create(&view(ID, 9), &mut buf()),
            Err(MetaError::AlreadyExists)
        ));
        // A keep-alive of a legacy view pins its index through the inventory.
        assert!(p.extend(ID, 8_000, None, &mut buf()).unwrap());
        let mut raw = buf();
        let n = b
            .store
            .read_register(&format!("{BASE}pit-states/{ID}"), &mut raw)
            .unwrap()
            .unwrap()
            .len;
        assert!(PointInTime::parse(&raw[..n]).unwrap().expires_at_millis == 8_000);
    }
}

#[test]
fn a_state_under_another_id_is_refused() {
    for b in all() {
        let (_, p) = pits(&b);
        let mut v = buf();
        let n = view(OTHER, 5_000).write(&mut v).unwrap();
        b.store
            .create_register(&format!("{BASE}pit-states/{ID}"), &v[..n])
            .unwrap();
        assert!(matches!(p.read(ID, &mut buf()), Err(MetaError::Format(_))));
    }
}

#[test]
fn a_legacy_entry_under_another_id_is_refused_and_not_adopted() {
    for b in all() {
        let (_, p) = pits(&b);
        let mut v = buf();
        let n = view(OTHER, 5_000).write(&mut v).unwrap();
        b.store
            .put(
                &format!("{BASE}pits/{ID}"),
                &mut SliceSource::new(&v[..n]),
                &mut [0u8; 4096],
            )
            .unwrap();
        assert!(matches!(p.read(ID, &mut buf()), Err(MetaError::Format(_))));
        let state = format!("{BASE}pit-states/{ID}");
        assert!(b.store.read_register(&state, &mut buf()).unwrap().is_none());
    }
}

#[test]
fn a_record_that_no_reader_would_take_is_not_stored() {
    for b in all() {
        let (_, p) = pits(&b);
        let mut no_shards = view(ID, 5_000);
        no_shards.shards.clear();
        assert!(
            p.create(&no_shards, &mut buf()).is_err(),
            "a view with no shards"
        );
        let mut twice = view(ID, 5_000);
        let file = twice.shards[0].files[0].clone();
        twice.shards[0].files.push(file);
        assert!(p.create(&twice, &mut buf()).is_err(), "a file twice");
        let state = format!("{BASE}pit-states/{ID}");
        assert!(
            b.store.read_register(&state, &mut buf()).unwrap().is_none(),
            "no register was left"
        );
        p.create(&capture(ID, 5_000), &mut buf()).unwrap();
        assert!(
            p.finish(&no_shards, &mut buf()).is_err(),
            "finishing with no shards"
        );
        assert!(
            p.release(ID, &mut buf()).unwrap(),
            "the capture is still releasable"
        );
    }
}

#[test]
fn keep_alive_finish_and_expiry_cost_what_the_spec_says() {
    for b in all() {
        let (now, p) = pits(&b);
        p.create(&capture(ID, 5_000), &mut buf()).unwrap();
        let (done, sent) = counted(b.meter(), || p.finish(&view(ID, 1), &mut buf()));
        done.unwrap();
        assert_eq!(
            sent,
            b.budget(&[(Get, 1), (Put, 1)], &[(Get, 1), (GetRange, 1), (Put, 1)]),
            "finish: {:?}",
            b.kind
        );
        let (moved, sent) = counted(b.meter(), || p.extend(ID, 8_000, None, &mut buf()).unwrap());
        assert!(moved);
        assert_eq!(
            sent,
            b.budget(&[(Get, 2), (Put, 1)], &[(Get, 2), (GetRange, 1), (Put, 1)]),
            "extend: {:?}",
            b.kind
        );
        let (held, sent) = counted(b.meter(), || p.extend(ID, 7_000, None, &mut buf()).unwrap());
        assert!(held);
        assert_eq!(
            sent,
            b.budget(&[(Get, 1)], &[(Get, 1)]),
            "no move: {:?}",
            b.kind
        );
        now.store(9_000, Ordering::SeqCst);
        let (reaped, sent) = counted(b.meter(), || p.reap(&mut buf()).unwrap());
        assert_eq!(reaped, 1);
        assert_eq!(
            sent,
            b.budget(
                &[(Get, 1), (Put, 1), (List, 1), (Delete, 1)],
                &[(Get, 1), (GetRange, 1), (Put, 1), (List, 1), (Delete, 1)]
            ),
            "reap: {:?}",
            b.kind
        );
    }
}

#[test]
fn a_keep_alive_rewrites_a_marker_whose_deadline_is_not_the_one_collectors_read_as_never() {
    for b in all() {
        let (_, p) = pits(&b);
        p.create(&view(ID, 5_000), &mut buf()).unwrap();
        // An older marker: a capture, but with a deadline that passes.
        let mut v = buf();
        let n = view(ID, 5_000)
            .marker()
            .with_expiry(1)
            .write(&mut v)
            .unwrap();
        b.store
            .put(
                &format!("{BASE}pits/{ID}"),
                &mut SliceSource::new(&v[..n]),
                &mut [0u8; 4096],
            )
            .unwrap();
        assert!(p.extend(ID, 8_000, None, &mut buf()).unwrap());
        let n = {
            let mut got = Vec::new();
            let mut sink = |c: &[u8]| {
                got.extend_from_slice(c);
                Ok(())
            };
            b.store
                .get(
                    &format!("{BASE}pits/{ID}"),
                    oss_blob::ByteRange::Full,
                    &mut [0u8; 4096],
                    &mut sink,
                )
                .unwrap();
            PointInTime::parse(&got).unwrap()
        };
        assert!(n.capturing);
        assert_eq!(
            n.expires_at_millis,
            i64::MAX as u64,
            "the marker pins until released"
        );
    }
}

#[test]
fn a_listing_longer_than_a_page_is_read_to_its_end() {
    let b = all().into_iter().next().unwrap();
    let (_, p) = pits(&b);
    for n in 0..1_001u32 {
        let id = format!("{n:022}");
        let mut v = buf();
        let len = view(&id, 5_000).write(&mut v).unwrap();
        b.store
            .put(
                &format!("{BASE}pits/{id}"),
                &mut SliceSource::new(&v[..len]),
                &mut [0u8; 4096],
            )
            .unwrap();
    }
    assert_eq!(p.live(&mut buf()).unwrap().len(), 1_001);
}
