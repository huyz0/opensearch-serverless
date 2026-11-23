//! docs/specs/bucket-commits/SPEC.md, over the in-process emulator with its
//! request meter.

// ARITH: test sizes and request counts are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{
    BucketManifest, BucketMember, ManifestCodec, ManifestEntry, bucket_manifest_key,
    encode_manifest, parse_bucket_member,
};
use oss_writer::bucket::{
    BucketCommits, BucketError, BucketStore, ManifestWrite, MemberOutcome, Swap,
};

mod bucket_support;

use bucket_support::Racing;

const BUCKET: &str = "bk-1";

fn commit(term: u64) -> Vec<u8> {
    let location = format!("t={term}");
    let files = [ManifestEntry {
        name: "segments_1",
        location,
        length: 10,
    }];
    encode_manifest(term, Some("nodeA"), &files, 3, -1).unwrap()
}

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

fn commits(store: Arc<dyn BlobStore>, write: ManifestWrite) -> BucketCommits {
    BucketCommits::new(BucketStore::new(store, "", write))
}

/// A bucket of members `m1`..`m3`, each at version 1 with a commit.
fn create_bucket(store: &Arc<dyn BlobStore>) {
    let mut manifest = BucketManifest::empty(BUCKET).unwrap().at_term(4).unwrap();
    for (uuid, name) in [("m1", "one"), ("m2", "two"), ("m3", "three")] {
        let member = BucketMember::first(name, Some(commit(1))).unwrap();
        manifest = manifest.with_member(uuid, member).unwrap();
    }
    let bucket = BucketStore::new(Arc::clone(store), "", ManifestWrite::Json);
    assert!(bucket.create(&manifest).unwrap());
}

fn swaps(list: &[(&str, u64, u64)]) -> BTreeMap<String, Swap> {
    list.iter()
        .map(|(uuid, version, term)| ((*uuid).to_owned(), Swap::new(*version, commit(*term))))
        .collect()
}

fn read_manifest(store: &Arc<dyn BlobStore>) -> (BucketManifest, u64) {
    let bucket = BucketStore::new(Arc::clone(store), "", ManifestWrite::Json);
    let read = bucket.read(BUCKET).unwrap().expect("bucket exists");
    (read.manifest, read.generation)
}

fn gets(e: &Emulator) -> u64 {
    e.meter().count(S3Op::Get)
}

fn puts(e: &Emulator) -> u64 {
    e.meter().count(S3Op::Put)
}

/// AC-1
#[test]
fn a_cold_swap_of_three_members_is_one_read_and_one_write() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let (g0, p0) = (gets(&e), puts(&e));
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    let out = c
        .swap_all(BUCKET, swaps(&[("m1", 1, 2), ("m2", 1, 2), ("m3", 1, 2)]))
        .unwrap();
    assert_eq!(out.len(), 3);
    assert!(out.values().all(|o| *o == MemberOutcome::Applied(2)));
    assert_eq!((gets(&e) - g0, puts(&e) - p0), (1, 1));
    let (manifest, _) = read_manifest(&store);
    for uuid in ["m1", "m2", "m3"] {
        let m = manifest.member(uuid).unwrap();
        assert_eq!(m.version, 2);
        assert_eq!(m.commit.as_deref(), Some(commit(2).as_slice()));
    }
    assert_eq!(manifest.term(), 4);
    assert_eq!(c.register_writes(), 1);
}

/// AC-2
#[test]
fn the_next_swap_reads_nothing() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap();
    let (g0, p0) = (gets(&e), puts(&e));
    let out = c.swap_all(BUCKET, swaps(&[("m1", 2, 3)])).unwrap();
    assert_eq!(out["m1"], MemberOutcome::Applied(3));
    assert_eq!((gets(&e) - g0, puts(&e) - p0), (0, 1));
}

/// AC-3
#[test]
fn a_stale_member_conflicts_and_its_siblings_still_land() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    let mut asked = swaps(&[("m1", 1, 2), ("m2", 7, 2)]);
    asked.extend(swaps(&[("ghost", 1, 2)]));
    let out = c.swap_all(BUCKET, asked).unwrap();
    assert_eq!(out["m1"], MemberOutcome::Applied(2));
    assert_eq!(out["m2"], MemberOutcome::Conflict(Some(1)));
    assert_eq!(out["ghost"], MemberOutcome::Conflict(None));
    let (manifest, _) = read_manifest(&store);
    assert_eq!(manifest.member("m1").unwrap().version, 2);
    assert_eq!(manifest.member("m2").unwrap().version, 1);
    assert_eq!(
        manifest.member("m2").unwrap().commit.as_deref(),
        Some(commit(1).as_slice())
    );
    assert!(manifest.member("ghost").is_none());

    // Nothing advances: nothing is written (a cached read is dropped and read once more).
    let p0 = puts(&e);
    let out = c.swap_all(BUCKET, swaps(&[("m2", 7, 3)])).unwrap();
    assert_eq!(out["m2"], MemberOutcome::Conflict(Some(1)));
    assert_eq!(puts(&e), p0);
    assert!(c.swap_all(BUCKET, BTreeMap::new()).unwrap().is_empty());
}

/// AC-4
#[test]
fn another_nodes_write_costs_one_read_and_one_retry() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let a = commits(Arc::clone(&store), ManifestWrite::Json);
    let b = commits(Arc::clone(&store), ManifestWrite::Json);
    a.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap();
    b.swap_all(BUCKET, swaps(&[("m2", 1, 2)])).unwrap();
    let (g0, p0) = (gets(&e), puts(&e));
    // a's cache is stale: m3 was not touched by b, so it is no conflict; m2 was.
    let out = a
        .swap_all(BUCKET, swaps(&[("m2", 1, 3), ("m3", 1, 3)]))
        .unwrap();
    assert_eq!(out["m3"], MemberOutcome::Applied(2));
    assert_eq!(out["m2"], MemberOutcome::Conflict(Some(2)));
    assert_eq!(gets(&e) - g0, 1);
    assert_eq!(
        a.register_writes(),
        3,
        "one at first, then the lost one and the retry"
    );
    assert!(puts(&e) - p0 >= 1);
}

/// AC-5
#[test]
fn sixteen_lost_swaps_give_up() {
    let e = emulator();
    create_bucket(&dynstore(&e));
    let racing: Arc<dyn BlobStore> = Arc::new(Racing {
        inner: Arc::clone(&e),
        races: AtomicUsize::new(usize::MAX),
    });
    let c = commits(racing, ManifestWrite::Json);
    let err = c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap_err();
    assert!(matches!(err, BucketError::Busy { attempts: 16 }), "{err:?}");
    assert_eq!(c.register_writes(), 16);
}

#[test]
fn a_swap_that_loses_fewer_than_sixteen_times_lands() {
    let e = emulator();
    create_bucket(&dynstore(&e));
    let racing: Arc<dyn BlobStore> = Arc::new(Racing {
        inner: Arc::clone(&e),
        races: AtomicUsize::new(15),
    });
    let c = commits(racing, ManifestWrite::Json);
    let out = c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap();
    assert_eq!(out["m1"], MemberOutcome::Applied(2));
    assert_eq!(c.register_writes(), 16);
}

/// AC-6
#[test]
fn a_missing_bucket_is_an_error_and_create_does_not_overwrite() {
    let e = emulator();
    let store = dynstore(&e);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    assert!(matches!(
        c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])),
        Err(BucketError::NoBucket { .. })
    ));
    create_bucket(&store);
    let (before, generation) = read_manifest(&store);
    let bucket = BucketStore::new(Arc::clone(&store), "", ManifestWrite::Json);
    let other = BucketManifest::empty(BUCKET).unwrap();
    assert!(!bucket.create(&other).unwrap());
    let (after, after_generation) = read_manifest(&store);
    assert_eq!((before, generation), (after, after_generation));
}

/// AC-7
#[test]
fn a_members_commit_is_read_without_the_rest() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    let read = c
        .member_commit(BUCKET, "m2")
        .unwrap()
        .expect("has a commit");
    assert_eq!(read.version, 1);
    assert_eq!(read.commit, commit(1));
    assert!(matches!(
        c.member_commit(BUCKET, "ghost"),
        Err(BucketError::NotAMember { .. })
    ));
    // A member with no commit yet.
    let mut manifest = read_manifest(&store).0;
    manifest = manifest
        .with_member("fresh", BucketMember::first("fresh", None).unwrap())
        .unwrap();
    let bucket = BucketStore::new(Arc::clone(&store), "", ManifestWrite::Json);
    let generation = read_manifest(&store).1;
    assert!(bucket.swap(&manifest, generation).unwrap().is_some());
    assert!(c.member_commit(BUCKET, "fresh").unwrap().is_none());
}

/// AC-8
#[test]
fn every_write_form_reads_back() {
    let forms = [
        (ManifestWrite::Json, b'{'),
        (ManifestWrite::Compact(ManifestCodec::Zstd), b'O'),
        (ManifestWrite::Auto(ManifestCodec::Zstd), b'O'),
    ];
    for (form, first) in forms {
        let e = emulator();
        let store = dynstore(&e);
        create_bucket(&store);
        let c = commits(Arc::clone(&store), form);
        c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let key = bucket_manifest_key("", BUCKET).unwrap();
        let read = store.read_register(&key, &mut buf).unwrap().unwrap();
        let value = &buf[..read.len];
        assert_eq!(value[0], first);
        let m = parse_bucket_member(value, "m1").unwrap().member.unwrap();
        assert_eq!((m.version, m.commit), (2, Some(commit(2))));
        assert_eq!(BucketManifest::parse(value).unwrap().len(), 3);
    }
}

#[test]
fn auto_compresses_only_a_large_manifest() {
    let e = emulator();
    let store = dynstore(&e);
    let mut manifest = BucketManifest::empty(BUCKET).unwrap();
    for i in 0..400 {
        let names: Vec<String> = (0..20).map(|n| format!("_{i}x{n}.cfs")).collect();
        let files: Vec<ManifestEntry<'_>> = names
            .iter()
            .map(|name| ManifestEntry {
                name,
                location: "t=1".to_owned(),
                length: 10,
            })
            .collect();
        let json = encode_manifest(1, Some("nodeA"), &files, 3, -1).unwrap();
        let member = BucketMember::first("idx", Some(json)).unwrap();
        manifest = manifest
            .with_member(&format!("uuid-{i:04}"), member)
            .unwrap();
    }
    let bucket = BucketStore::new(Arc::clone(&store), "", ManifestWrite::Json);
    assert!(bucket.create(&manifest).unwrap());
    let c = commits(Arc::clone(&store), ManifestWrite::Auto(ManifestCodec::Zstd));
    c.swap_all(BUCKET, swaps(&[("uuid-0000", 1, 2)])).unwrap();
    let mut buf = vec![0u8; 1 << 20];
    let key = bucket_manifest_key("", BUCKET).unwrap();
    let read = store.read_register(&key, &mut buf).unwrap().unwrap();
    assert_eq!(&buf[..4], b"OSBM");
    assert_eq!(buf[5], 2, "zstd");
    assert!(read.len < 400 * 200);
}

/// AC-9
#[test]
fn threads_swapping_different_members_all_land() {
    let e = emulator();
    let store = dynstore(&e);
    let mut manifest = BucketManifest::empty(BUCKET).unwrap();
    for i in 0..8 {
        let member = BucketMember::first("idx", Some(commit(1))).unwrap();
        manifest = manifest.with_member(&format!("m{i}"), member).unwrap();
    }
    BucketStore::new(Arc::clone(&store), "", ManifestWrite::Json)
        .create(&manifest)
        .unwrap();
    let c = Arc::new(commits(Arc::clone(&store), ManifestWrite::Json));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let c = Arc::clone(&c);
            std::thread::spawn(move || {
                let uuid = format!("m{i}");
                c.swap_all(BUCKET, swaps(&[(&uuid, 1, 2)])).unwrap()
            })
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        let out = h.join().unwrap();
        assert_eq!(out[&format!("m{i}")], MemberOutcome::Applied(2));
    }
    let (manifest, _) = read_manifest(&store);
    assert!(manifest.members().all(|(_, m)| m.version == 2));
    assert!(c.register_writes() <= 8);
}

/// AC-10
#[test]
fn a_commit_that_is_not_a_manifest_is_refused_before_any_request() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    let (g0, p0) = (gets(&e), puts(&e));
    let mut bad = BTreeMap::new();
    bad.insert("m1".to_owned(), Swap::new(1, b"{}".to_vec()));
    assert!(matches!(
        c.swap_all(BUCKET, bad),
        Err(BucketError::Format(_))
    ));
    assert_eq!((gets(&e), puts(&e)), (g0, p0));
}

#[test]
fn forget_drops_the_cache_so_the_next_swap_reads_again() {
    let e = emulator();
    let store = dynstore(&e);
    create_bucket(&store);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap();
    let g0 = gets(&e);
    c.forget(BUCKET);
    c.swap_all(BUCKET, swaps(&[("m1", 2, 3)])).unwrap();
    assert_eq!(gets(&e) - g0, 1);
}

#[test]
fn errors_and_handles_describe_themselves() {
    let e = emulator();
    let store = dynstore(&e);
    let c = commits(Arc::clone(&store), ManifestWrite::Json);
    let debug = format!("{c:?}");
    assert!(
        debug.contains("BucketCommits") && debug.contains("BucketStore"),
        "{debug}"
    );
    let missing = c.swap_all(BUCKET, swaps(&[("m1", 1, 2)])).unwrap_err();
    assert!(missing.to_string().contains("bk-1"), "{missing}");
    let ghost = BucketError::NotAMember {
        bucket: "bk-1".to_owned(),
        uuid: "ghost".to_owned(),
    };
    assert_eq!(
        ghost.to_string(),
        "index [ghost] is not a member of bucket [bk-1]"
    );
    assert!(
        BucketError::Busy { attempts: 16 }
            .to_string()
            .contains("16")
    );
}
