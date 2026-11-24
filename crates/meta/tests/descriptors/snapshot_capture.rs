//! docs/specs/snapshot-take AC-2, AC-3: describing an index and reading each
//! shard's published commit for a snapshot record, per backend.

use oss_blob::S3Op::{Get, GetRange, List};
use oss_format::IndexRecord;
use oss_meta::{CaptureError, DeleteMode, SnapshotTaker};

use crate::point_in_time::buf;
use crate::point_in_time_bucket::{bucketed_index, node_commit, publish_bucket};
use crate::point_in_time_capture::{index, publish};
use crate::{BASE, Backend, all, counted};

fn taker(b: &Backend) -> SnapshotTaker {
    SnapshotTaker::new(std::sync::Arc::clone(&b.store), BASE)
}

fn gets(n: &[(oss_blob::S3Op, u64)]) -> u64 {
    n.iter()
        .filter(|(op, _)| matches!(op, Get | GetRange))
        .map(|(_, c)| c)
        .sum()
}

#[test]
fn an_index_is_described_and_each_shards_commit_is_read_for_one_get_each() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        index(&d, "logs", "u1", 2);
        publish(&*b.store, "logs", "u1", 0, 3);
        publish(&*b.store, "logs", "u1", 1, 4);
        let (described, n) = counted(b.meter(), || t.describe(&d, "logs", &mut buf()).unwrap());
        assert_eq!(gets(&n), 1, "{:?} {n:?}", b.kind);
        assert_eq!(described.uuid, "u1");
        assert_eq!(
            (described.number_of_shards, described.shard_count()),
            (2, 2)
        );
        assert_eq!(described.units(), [0, 1]);
        let (shard, n) = counted(b.meter(), || {
            t.capture_shard(&described, 1, &mut buf()).unwrap()
        });
        assert_eq!(gets(&n), 1, "{:?} {n:?}", b.kind);
        assert!(n.iter().all(|(op, _)| *op != List), "{:?}", b.kind);
        assert_eq!((shard.shard, shard.term), (1, 4));
        assert_eq!(
            shard.files,
            [
                ("_0.cfs".to_owned(), "t=1/s0#10:5".to_owned()),
                ("_0.si".to_owned(), "t=1".to_owned())
            ]
        );
        assert_eq!(
            shard.lengths,
            [("_0.cfs".to_owned(), 5), ("_0.si".to_owned(), 7)]
        );
        let (_, n) = counted(b.meter(), || t.verify(&d, &described, &mut buf()).unwrap());
        assert_eq!(gets(&n), 1, "{:?} {n:?}", b.kind);
    }
}

#[test]
fn a_missing_index_an_alias_and_an_unpublished_shard_are_refused() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        assert!(
            matches!(
                t.describe(&d, "nope", &mut buf()),
                Err(CaptureError::NoSuchIndex)
            ),
            "{:?}",
            b.kind
        );
        index(&d, "logs", "u1", 2);
        publish(&*b.store, "logs", "u1", 0, 3);
        let described = t.describe(&d, "logs", &mut buf()).unwrap();
        assert!(
            matches!(
                t.capture_shard(&described, 1, &mut buf()),
                Err(CaptureError::NothingPublished(1))
            ),
            "{:?}",
            b.kind
        );
        d.create_alias(&oss_meta::AliasRecord::new("al", &["logs"]), 1, &mut buf())
            .unwrap();
        assert!(
            matches!(t.describe(&d, "al", &mut buf()), Err(CaptureError::IsAlias)),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn an_index_deleted_or_created_again_is_not_the_one_described() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        index(&d, "logs", "u1", 1);
        let described = t.describe(&d, "logs", &mut buf()).unwrap();
        d.delete("logs").unwrap();
        assert!(
            matches!(
                t.verify(&d, &described, &mut buf()),
                Err(CaptureError::Replaced)
            ),
            "{:?} deleted",
            b.kind
        );
        index(&d, "logs", "u2", 1);
        assert!(
            matches!(
                t.verify(&d, &described, &mut buf()),
                Err(CaptureError::Replaced)
            ),
            "{:?} recreated",
            b.kind
        );
    }
}

#[test]
fn an_index_with_retired_shards_is_refused() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        // Two live units in four slots.
        let record = IndexRecord {
            unit_layout: Some(crate::point_in_time_capture::LAYOUT),
            ..IndexRecord::new("split", "u1", 4)
        };
        d.create(&record, 1, &mut buf()).unwrap();
        let out = t.describe(&d, "split", &mut buf());
        assert!(
            matches!(out, Err(CaptureError::Unsupported(_))),
            "{:?} {out:?}",
            b.kind
        );
    }
}

#[test]
fn a_bucketed_member_is_read_from_its_bucket_for_one_get() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        bucketed_index(&d, "small", "um", "bk");
        publish_bucket(&*b.store, "bk", &[("um", Some(&node_commit(5, true)))]);
        let described = t.describe(&d, "small", &mut buf()).unwrap();
        assert_eq!(described.units(), [0]);
        let (shard, n) = counted(b.meter(), || {
            t.capture_shard(&described, 0, &mut buf()).unwrap()
        });
        assert_eq!((shard.shard, shard.term), (0, 5), "{:?}", b.kind);
        assert_eq!(gets(&n), 1, "{:?} {n:?}", b.kind);
        assert_eq!(shard.files.len(), 2, "{:?}", b.kind);
        // A member with no commit has published nothing.
        bucketed_index(&d, "fresh", "uf", "bk2");
        publish_bucket(&*b.store, "bk2", &[("uf", None)]);
        let fresh = t.describe(&d, "fresh", &mut buf()).unwrap();
        assert!(
            matches!(
                t.capture_shard(&fresh, 0, &mut buf()),
                Err(CaptureError::NothingPublished(0))
            ),
            "{:?}",
            b.kind
        );
    }
}
