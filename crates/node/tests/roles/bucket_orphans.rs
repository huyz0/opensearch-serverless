//! docs/specs/bucket-member-orphans: a member no index owns is taken out
//! after the grace, and one an index owns never is.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::{LeaseClock, SystemClock};
use oss_format::{BucketManifest, BucketMember, IndexRecord};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::writer::{BucketMemberSweep, BucketWiring};
use oss_writer::bucket::{BucketStore, Limits, ManifestWrite};

const GRACE: i64 = 1_000;

struct Rig {
    store: Arc<Emulator>,
    wiring: Arc<BucketWiring>,
    descriptors: Arc<DescriptorStore>,
}

fn dynstore(store: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(store) as Arc<dyn BlobStore>
}

/// Bucket `b-1` holds `live` (descriptor names it), `gone` (no descriptor) and
/// `moved` (a descriptor of another uuid); bucket `b-2` holds `lone`, gone.
fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 7));
    let descriptors = Arc::new(
        DescriptorStore::new(dynstore(&store), "", DeleteMode::Tombstone).expect("descriptors"),
    );
    let mut buf = vec![0u8; 64 << 10];
    for (name, uuid) in [("live", "u-live"), ("moved", "u-new")] {
        let mut record = IndexRecord::new(name, uuid, 1);
        record.bucket = Some("b-1".into());
        descriptors.create(&record, 1, &mut buf).expect("created");
    }
    let write = ManifestWrite::Json;
    let make = |id: &str, members: &[(&str, &str)]| {
        let mut manifest = BucketManifest::empty(id).unwrap().at_term(1).unwrap();
        for (uuid, name) in members {
            manifest = manifest
                .with_member(uuid, BucketMember::first(name, None).unwrap())
                .unwrap();
        }
        assert!(
            BucketStore::new(dynstore(&store), "", write)
                .create(&manifest)
                .unwrap()
        );
    };
    make(
        "b-1",
        &[("u-live", "live"), ("u-gone", "gone"), ("u-old", "moved")],
    );
    make("b-2", &[("u-lone", "lone")]);
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let wiring = Arc::new(BucketWiring::new(
        dynstore(&store),
        "",
        Arc::clone(&descriptors),
        clock,
        write,
        Limits::new(1 << 20, 8, 1 << 16),
    ));
    Rig {
        store,
        wiring,
        descriptors,
    }
}

fn sweep(r: &Rig, dry_run: bool) -> BucketMemberSweep {
    let descriptors =
        DescriptorStore::new(dynstore(&r.store), "", DeleteMode::Tombstone).expect("descriptors");
    BucketMemberSweep::new(
        Arc::clone(&r.wiring),
        dynstore(&r.store),
        descriptors,
        "",
        GRACE,
        dry_run,
    )
}

fn members(r: &Rig, bucket: &str) -> Vec<String> {
    let mut found: Vec<String> = r
        .wiring
        .members(bucket)
        .expect("reads")
        .expect("bucket")
        .into_iter()
        .map(|(uuid, _)| uuid)
        .collect();
    found.sort();
    found
}

// docs/specs/bucket-member-orphans AC-1
#[test]
fn a_member_is_taken_out_only_at_the_second_sighting_after_the_grace() {
    let r = rig();
    let sweep = sweep(&r, false);
    let first = sweep.slice(0).expect("slice");
    assert_eq!((first.buckets, first.members), (2, 4));
    assert_eq!((first.candidates, first.removed), (3, 0));
    assert_eq!(members(&r, "b-1").len(), 3, "seen once, kept");
    let early = sweep.slice(GRACE - 1).expect("slice");
    assert_eq!((early.candidates, early.removed), (3, 0));
    let late = sweep.slice(GRACE).expect("slice");
    assert_eq!(late.removed, 3);
    assert_eq!(members(&r, "b-1"), ["u-live"]);
    assert!(members(&r, "b-2").is_empty());
}

// docs/specs/bucket-member-orphans AC-2
#[test]
fn a_member_whose_index_appears_before_the_grace_is_kept() {
    let r = rig();
    let sweep = sweep(&r, false);
    sweep.slice(0).expect("slice");
    // The create that was joining writes its descriptor.
    let mut record = IndexRecord::new("gone", "u-gone", 1);
    record.bucket = Some("b-1".into());
    r.descriptors
        .create(&record, 1, &mut vec![0u8; 64 << 10])
        .expect("created");
    assert_eq!(sweep.slice(GRACE).expect("slice").removed, 2);
    assert_eq!(members(&r, "b-1"), ["u-gone", "u-live"]);
    // It stays kept at every later pass.
    sweep.slice(GRACE * 5).expect("slice");
    assert_eq!(members(&r, "b-1"), ["u-gone", "u-live"]);
}

// docs/specs/bucket-member-orphans AC-3
#[test]
fn dry_run_counts_and_removes_nothing() {
    let r = rig();
    let sweep = sweep(&r, true);
    sweep.slice(0).expect("slice");
    assert_eq!(sweep.slice(GRACE).expect("slice").removed, 3);
    assert_eq!(members(&r, "b-1").len(), 3);
}

// docs/specs/bucket-member-orphans AC-4
#[test]
fn a_first_sighting_writes_nothing_and_lists_once() {
    let r = rig();
    let sweep = sweep(&r, false);
    let meter = r.store.meter();
    let puts = meter.count(S3Op::Put);
    let lists = meter.count(S3Op::List);
    sweep.slice(0).expect("slice");
    assert_eq!(meter.count(S3Op::Put), puts);
    assert_eq!(
        meter.count(S3Op::List) - lists,
        3,
        "two buckets, one key each, plus the short page"
    );
}

// docs/specs/bucket-member-orphans AC-5
#[test]
fn the_cursor_moves_four_buckets_at_a_time_and_wraps() {
    let r = rig();
    for n in 3..=7 {
        let id = format!("b-{n}");
        let manifest = BucketManifest::empty(&id).unwrap().at_term(1).unwrap();
        assert!(
            BucketStore::new(dynstore(&r.store), "", ManifestWrite::Json)
                .create(&manifest)
                .unwrap()
        );
    }
    let sweep = sweep(&r, false);
    let seen: Vec<usize> = (0..3)
        .map(|i| sweep.slice(i).expect("slice").buckets)
        .collect();
    assert_eq!(
        seen,
        [4, 3, 4],
        "7 buckets: four, the last three, then around again"
    );
}
