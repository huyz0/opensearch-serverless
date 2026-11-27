//! docs/specs/bucket-assignment/SPEC.md, over the in-process emulator with
//! its request meter.

// ARITH: test sizes and request counts are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use crate::bucket_support;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use bucket_support::Racing;
use oss_blob::{BlobStore, ByteRange, Emulator, S3Op};
use oss_format::{
    BucketManifest, BucketMember, ManifestEntry, bucket_manifest_key, bucket_open_key,
    encode_manifest,
};
use oss_writer::bucket::{
    BucketAssigner, BucketCommits, BucketError, BucketStore, Limits, ManifestWrite, STRIPES, Swap,
};

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

fn bucket_store(store: &Arc<dyn BlobStore>) -> BucketStore {
    BucketStore::new(Arc::clone(store), "", ManifestWrite::Json)
}

fn assigner(store: &Arc<dyn BlobStore>, limits: Limits) -> BucketAssigner {
    BucketAssigner::new(bucket_store(store), limits)
}

fn roomy() -> Limits {
    Limits::new(u64::MAX, 1024, usize::MAX)
}

fn gets(e: &Emulator) -> u64 {
    e.meter().count(S3Op::Get)
}

fn puts(e: &Emulator) -> u64 {
    e.meter().count(S3Op::Put)
}

fn manifest(store: &Arc<dyn BlobStore>, bucket: &str) -> BucketManifest {
    bucket_store(store)
        .read(bucket)
        .unwrap()
        .expect("bucket exists")
        .manifest
}

fn open_value(e: &Emulator, stripe: usize) -> Option<String> {
    let key = bucket_open_key("", stripe).unwrap();
    let mut buf = vec![0u8; 64];
    let read = e.read_register(&key, &mut buf).unwrap()?;
    Some(String::from_utf8(buf[..read.len].to_vec()).unwrap())
}

/// A group on stripe 2 (its hash is the Java's recorded `cbd8a7b341bd9b02`).
const GROUP: &str = "hello";

/// AC-1
#[test]
fn a_stripe_is_the_low_four_bits_of_the_groups_hash() {
    assert_eq!(BucketAssigner::stripe("hello"), 2);
    for group in [
        "a",
        "tenant-1",
        "tenant-2",
        "",
        "日本語",
        "x".repeat(300).as_str(),
    ] {
        assert!(BucketAssigner::stripe(group) < STRIPES);
    }
    let distinct: std::collections::BTreeSet<usize> = (0..200)
        .map(|i| BucketAssigner::stripe(&format!("tenant-{i}")))
        .collect();
    assert_eq!(distinct.len(), STRIPES, "200 groups reach every stripe");
    assert_eq!(BucketAssigner::bucket_id(2, 0), "s02-0");
    assert_eq!(BucketAssigner::bucket_id(11, 37), "s11-37");
    assert_eq!(BucketAssigner::bucket_id(0, 100), "s00-100");
}

/// AC-2
#[test]
fn the_first_join_creates_the_bucket() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, roomy());
    let (g0, p0) = (gets(&e), puts(&e));
    let bucket = a.join(GROUP, "logs-1", "uuid-1").unwrap();
    assert_eq!(bucket, "s02-0");
    assert_eq!((gets(&e) - g0, puts(&e) - p0), (2, 1));
    let m = manifest(&store, &bucket);
    let member = m.member("uuid-1").unwrap();
    assert_eq!(
        (
            member.index_name.as_str(),
            member.version,
            member.commit.is_none()
        ),
        ("logs-1", 0, true)
    );
    assert_eq!(open_value(&e, 2), None, "the open register is not written");
}

/// AC-3
#[test]
fn joining_twice_writes_nothing_the_second_time() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, roomy());
    a.join(GROUP, "logs-1", "uuid-1").unwrap();
    let p0 = puts(&e);
    assert_eq!(a.join(GROUP, "logs-1", "uuid-1").unwrap(), "s02-0");
    assert_eq!(puts(&e), p0);
}

/// AC-4
#[test]
fn a_groups_indices_share_a_bucket_one_conditional_write_each() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, roomy());
    a.join(GROUP, "i0", "u0").unwrap();
    for i in 1..5 {
        let (g0, p0) = (gets(&e), puts(&e));
        let bucket = a.join(GROUP, &format!("i{i}"), &format!("u{i}")).unwrap();
        assert_eq!(bucket, "s02-0");
        assert_eq!((gets(&e) - g0, puts(&e) - p0), (2, 1));
    }
    assert_eq!(manifest(&store, "s02-0").len(), 5);
}

/// AC-5
#[test]
fn a_full_bucket_moves_the_stripe_to_the_next_one() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, Limits::new(u64::MAX, 2, usize::MAX));
    assert_eq!(a.join(GROUP, "i0", "u0").unwrap(), "s02-0");
    assert_eq!(a.join(GROUP, "i1", "u1").unwrap(), "s02-0");
    assert_eq!(open_value(&e, 2), None);
    assert_eq!(a.join(GROUP, "i2", "u2").unwrap(), "s02-1");
    assert_eq!(open_value(&e, 2).as_deref(), Some("1"));
    assert_eq!(manifest(&store, "s02-0").len(), 2);
    assert_eq!(manifest(&store, "s02-1").len(), 1);
    // The open register is now read, and the next bucket fills and moves on.
    assert_eq!(a.join(GROUP, "i3", "u3").unwrap(), "s02-1");
    assert_eq!(a.join(GROUP, "i4", "u4").unwrap(), "s02-2");
    assert_eq!(open_value(&e, 2).as_deref(), Some("2"));
}

/// A bucket whose members have commits of known sizes.
fn bucket_with_data(store: &Arc<dyn BlobStore>, bytes: u64) {
    let a = assigner(store, roomy());
    a.join(GROUP, "i0", "u0").unwrap();
    let files = [ManifestEntry {
        name: "segments_1",
        location: "t=1".to_owned(),
        length: bytes,
    }];
    let commit = encode_manifest(1, Some("nodeA"), &files, 1, -1).unwrap();
    let commits = BucketCommits::new(bucket_store(store));
    let mut swaps = std::collections::BTreeMap::new();
    swaps.insert("u0".to_owned(), Swap::new(0, commit));
    commits.swap_all("s02-0", swaps).unwrap();
}

#[test]
fn a_bucket_is_full_by_data_size_at_the_target() {
    for (target, expected) in [(101, "s02-0"), (100, "s02-1"), (99, "s02-1")] {
        let e = emulator();
        let store = dynstore(&e);
        bucket_with_data(&store, 100);
        let a = assigner(&store, Limits::new(target, 1024, usize::MAX));
        assert_eq!(
            a.join(GROUP, "i1", "u1").unwrap(),
            expected,
            "target {target}"
        );
    }
}

#[test]
fn a_bucket_is_full_by_stored_manifest_size_at_the_limit() {
    let e = emulator();
    let store = dynstore(&e);
    bucket_with_data(&store, 1);
    let key = bucket_manifest_key("", "s02-0").unwrap();
    let mut buf = vec![0u8; 1 << 20];
    let stored = e.read_register(&key, &mut buf).unwrap().unwrap().len;
    let read = bucket_store(&store).read("s02-0").unwrap().unwrap();
    assert_eq!(read.stored_bytes, stored);
    for (limit, expected) in [(stored + 1, "s02-0"), (stored, "s02-1")] {
        let e = emulator();
        let store = dynstore(&e);
        bucket_with_data(&store, 1);
        let a = assigner(&store, Limits::new(u64::MAX, 1024, limit));
        assert_eq!(
            a.join(GROUP, "i1", "u1").unwrap(),
            expected,
            "limit {limit}"
        );
    }
}

#[test]
fn a_bucket_one_under_the_member_limit_still_takes_one() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, Limits::new(u64::MAX, 3, usize::MAX));
    for i in 0..3 {
        assert_eq!(
            a.join(GROUP, &format!("i{i}"), &format!("u{i}")).unwrap(),
            "s02-0"
        );
    }
    assert_eq!(a.join(GROUP, "i3", "u3").unwrap(), "s02-1");
}

/// AC-6
#[test]
fn a_lost_manifest_swap_is_retried_and_sixteen_are_busy() {
    let e = emulator();
    assigner(&dynstore(&e), roomy())
        .join(GROUP, "i0", "u0")
        .unwrap();
    for (races, ok) in [(3usize, true), (usize::MAX, false)] {
        let racing: Arc<dyn BlobStore> = Arc::new(Racing {
            inner: Arc::clone(&e),
            races: AtomicUsize::new(races),
        });
        let a = assigner(&racing, roomy());
        let name = format!("race-{races}");
        let result = a.join(GROUP, &name, &name);
        if ok {
            assert_eq!(result.unwrap(), "s02-0");
            assert!(manifest(&dynstore(&e), "s02-0").member(&name).is_some());
        } else {
            assert!(matches!(result, Err(BucketError::Busy { attempts: 16 })));
        }
    }
}

/// AC-7
#[test]
fn leave_removes_only_that_member_and_tolerates_the_absent() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, roomy());
    a.join(GROUP, "i0", "u0").unwrap();
    a.join(GROUP, "i1", "u1").unwrap();
    a.leave("s02-0", "u0").unwrap();
    let m = manifest(&store, "s02-0");
    assert!(m.member("u0").is_none() && m.member("u1").is_some());
    let p0 = puts(&e);
    a.leave("s02-0", "u0").unwrap();
    a.leave("s09-0", "nobody").unwrap();
    assert_eq!(puts(&e), p0);
}

/// bucket-member-orphans: the members of a bucket, uuid and index name.
#[test]
fn members_lists_uuid_and_name_and_an_absent_bucket_has_none() {
    let e = emulator();
    let store = dynstore(&e);
    let a = assigner(&store, roomy());
    a.join(GROUP, "i0", "u0").unwrap();
    a.join(GROUP, "i1", "u1").unwrap();
    let mut found = a.members("s02-0").unwrap().expect("bucket");
    found.sort();
    assert_eq!(
        found,
        [("u0".into(), "i0".into()), ("u1".into(), "i1".into())]
    );
    assert_eq!(a.members("s09-0").unwrap(), None);
}

/// AC-8
#[test]
fn limits_are_clamped() {
    let l = Limits::new(0, 0, 0);
    assert_eq!(
        (l.target_bytes(), l.max_members(), l.max_manifest_bytes()),
        (1, 1, 1)
    );
    assert_eq!(Limits::new(5, 5000, 7).max_members(), 1024);
    assert_eq!(Limits::new(5, 1024, 7).max_members(), 1024);
    assert_eq!(Limits::new(5, 17, 7).max_members(), 17);
    let l = Limits::new(5, 17, 7);
    assert_eq!((l.target_bytes(), l.max_manifest_bytes()), (5, 7));
    let _ = (BucketMember::first("x", None).unwrap(), ByteRange::Full);
}

/// A manifest read through the assigner's store sees what it wrote.
#[test]
fn the_open_register_round_trips_through_the_store() {
    let e = emulator();
    let store = dynstore(&e);
    let bucket = bucket_store(&store);
    assert!(bucket.read_open(4).unwrap().is_none());
    assert!(bucket.swap_open(4, 3, 0).unwrap());
    let read = bucket.read_open(4).unwrap().unwrap();
    assert_eq!(read.bucket_generation, 3);
    assert!(!bucket.swap_open(4, 9, 0).unwrap(), "created already");
    assert!(bucket.swap_open(4, 9, read.generation).unwrap());
    assert_eq!(bucket.read_open(4).unwrap().unwrap().bucket_generation, 9);
    assert!(
        !bucket.swap_open(4, 10, read.generation).unwrap(),
        "stale generation"
    );
    assert_eq!(open_value(&e, 4).as_deref(), Some("9"));
}
