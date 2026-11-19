//! docs/specs/janitor-point-in-time-pins/SPEC.md: the pins points in time
//! keep, read from the store and judged by a sweep.

// ARITH: counts here are test constants and small sums.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{
    FileLocation, ManifestEntry, PitFile, PitShard, PointInTime, ShardRef, encode_manifest,
};
use oss_janitor::gc::{Collector, Config, Memory, Pinned, Pins, PointInTimePins};
use oss_meta::PointInTimeStore;

const BASE: &str = "d/";
const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const OTHER: &str = "BBBBBBBBBBBBBBBBBBBBBB";
const SHARD: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 0,
};
const PATH: &str = "d/segments/logs#u1#0/";
const LIVE: Config = Config {
    dry_run: false,
    minimum_unreferenced_millis: 0,
    max_watched: 1000,
    max_pinned: 1000,
};

struct Rig {
    emulator: Arc<Emulator>,
    store: PointInTimeStore,
    now: Arc<AtomicU64>,
}

impl Rig {
    fn new() -> Self {
        let emulator = Arc::new(Emulator::new(64 << 20, 1));
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = Arc::clone(&now);
        let store = PointInTimeStore::new(
            Arc::clone(&emulator) as Arc<dyn BlobStore>,
            BASE,
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .unwrap();
        Self {
            emulator,
            store,
            now,
        }
    }

    fn create(&self, pit: &PointInTime) {
        self.store.create(pit, &mut vec![0u8; 1 << 16]).unwrap();
    }

    fn snapshot(&self) -> PointInTimePins {
        PointInTimePins::snapshot(&self.store, BASE, 1000, &mut vec![0u8; 1 << 16]).unwrap()
    }

    fn count(&self, op: S3Op) -> u64 {
        self.emulator.meter().count(op)
    }
}

fn file(name: &str, location: &str) -> PitFile {
    PitFile {
        name: name.to_owned(),
        location: location.to_owned(),
        length: Some(5),
    }
}

fn view(id: &str, index: &str, uuid: Option<&str>, shard: u32, files: Vec<PitFile>) -> PointInTime {
    PointInTime {
        id: id.to_owned(),
        index: index.to_owned(),
        index_uuid: uuid.map(str::to_owned),
        expires_at_millis: 9_000,
        capturing: false,
        shards: vec![PitShard {
            shard,
            term: 1,
            files,
        }],
    }
}

fn logs_view(id: &str, files: Vec<PitFile>) -> PointInTime {
    view(id, "logs", Some("u1"), 0, files)
}

fn keys(pins: &PointInTimePins) -> Vec<String> {
    match pins.pinned(&SHARD).unwrap() {
        Pinned::Blobs(mut keys) => {
            keys.sort();
            keys
        }
        Pinned::Everything => panic!("expected blobs"),
    }
}

#[test]
fn no_views_pin_no_blobs() {
    let rig = Rig::new();
    assert_eq!(keys(&rig.snapshot()), Vec::<String>::new());
}

#[test]
fn a_view_pins_each_blob_its_files_are_in_whatever_the_location_form() {
    let rig = Rig::new();
    rig.create(&logs_view(
        ID,
        vec![
            file("_0.si", "t=1"),
            file("_0.cfs", "t=1/seg0#10:5"),
            file(
                "_1.cfs",
                "@n1/e=00000000000000000003/00000000000000000007#0:5",
            ),
        ],
    ));
    assert_eq!(
        keys(&rig.snapshot()),
        [
            "d/packs/n1/e=00000000000000000003/00000000000000000007".to_owned(),
            format!("{PATH}t=1/_0.si"),
            format!("{PATH}t=1/seg0"),
        ]
    );
}

#[test]
fn views_of_another_index_incarnation_or_shard_pin_nothing_here() {
    let rig = Rig::new();
    let files = || vec![file("_0.si", "t=1")];
    rig.create(&view(ID, "other", Some("u1"), 0, files()));
    rig.create(&view(OTHER, "logs", Some("u2"), 0, files()));
    rig.create(&view(
        "CCCCCCCCCCCCCCCCCCCCCC",
        "logs",
        Some("u1"),
        1,
        files(),
    ));
    assert!(keys(&rig.snapshot()).is_empty());
}

#[test]
fn a_view_written_without_a_uuid_pins_by_name() {
    let rig = Rig::new();
    rig.create(&view(ID, "logs", None, 0, vec![file("_0.si", "t=1")]));
    assert_eq!(keys(&rig.snapshot()), [format!("{PATH}t=1/_0.si")]);
}

#[test]
fn the_files_of_several_views_are_all_pinned() {
    let rig = Rig::new();
    rig.create(&logs_view(ID, vec![file("_0.si", "t=1")]));
    rig.create(&logs_view(OTHER, vec![file("_1.si", "t=2")]));
    assert_eq!(
        keys(&rig.snapshot()),
        [format!("{PATH}t=1/_0.si"), format!("{PATH}t=2/_1.si")]
    );
}

#[test]
fn an_unfinished_capture_of_the_index_pins_everything_and_one_of_another_does_not() {
    let rig = Rig::new();
    let mut marker = logs_view(ID, Vec::new());
    marker.capturing = true;
    marker.shards.clear();
    rig.create(&marker);
    assert_eq!(rig.snapshot().pinned(&SHARD).unwrap(), Pinned::Everything);
    let other = ShardRef {
        index: "other",
        uuid: "u9",
        shard: 0,
    };
    assert_eq!(
        rig.snapshot().pinned(&other).unwrap(),
        Pinned::Blobs(Vec::new())
    );
}

#[test]
fn a_view_that_cannot_be_read_pins_everything() {
    let rig = Rig::new();
    rig.emulator
        .create_register(&format!("d/pit-states/{ID}"), b"not json")
        .unwrap();
    rig.emulator
        .put(
            &format!("d/pits/{ID}"),
            &mut SliceSource::new(b"x"),
            &mut [0u8; 16],
        )
        .unwrap();
    assert_eq!(rig.snapshot().pinned(&SHARD).unwrap(), Pinned::Everything);
}

#[test]
fn a_location_no_reader_could_follow_pins_everything() {
    let pins = PointInTimePins::of(BASE, vec![logs_view(ID, vec![file("_0.si", "t=1/a#b:c")])]);
    assert_eq!(pins.pinned(&SHARD).unwrap(), Pinned::Everything);
    let pins = PointInTimePins::of(BASE, vec![logs_view(ID, vec![file("_0.si", "t=../x")])]);
    assert_eq!(pins.pinned(&SHARD).unwrap(), Pinned::Everything);
}

#[test]
fn a_snapshot_costs_a_listing_and_a_read_a_view_and_answering_costs_nothing() {
    let rig = Rig::new();
    rig.create(&logs_view(ID, vec![file("_0.si", "t=1")]));
    rig.create(&logs_view(OTHER, vec![file("_1.si", "t=2")]));
    let (lists, gets) = (rig.count(S3Op::List), rig.count(S3Op::Get));
    let pins = rig.snapshot();
    assert_eq!(rig.count(S3Op::List) - lists, 1);
    assert_eq!(rig.count(S3Op::Get) - gets, 2);
    let all: Vec<u64> = S3Op::ALL.iter().map(|op| rig.count(*op)).collect();
    for shard in 0..50 {
        let shard = ShardRef { shard, ..SHARD };
        pins.pinned(&shard).unwrap();
    }
    let after: Vec<u64> = S3Op::ALL.iter().map(|op| rig.count(*op)).collect();
    assert_eq!(all, after, "no request per shard");
}

#[test]
fn a_sweep_keeps_what_a_view_names_until_the_view_is_released() {
    let rig = Rig::new();
    let put = |rest: &str| {
        rig.emulator
            .put(
                &format!("{PATH}{rest}"),
                &mut SliceSource::new(b"bytes"),
                &mut [0u8; 64],
            )
            .unwrap();
    };
    put("t=1/_0.seg");
    put("t=1/_9.seg");
    put("t=2/_1.seg");
    let entry = ManifestEntry {
        name: "_1.cfs",
        location: FileLocation::Whole { term_dir: "t=2" }.encode(),
        length: 5,
    };
    let manifest = encode_manifest(2, Some("n1"), &[entry], -1, -1).unwrap();
    rig.emulator
        .create_register(&format!("{PATH}manifest"), &manifest)
        .unwrap();
    rig.create(&logs_view(ID, vec![file("_0.seg", "t=1")]));

    let mut memory = Memory::new();
    let sweep = |pins: &PointInTimePins, memory: &mut Memory| {
        let clock = || 1_000;
        let collector = Collector::new(&*rig.emulator, BASE, pins, &clock, LIVE);
        collector
            .sweep(SHARD, memory, &mut vec![0u8; 1 << 16])
            .unwrap()
    };
    sweep(&rig.snapshot(), &mut memory);
    let second = sweep(&rig.snapshot(), &mut memory);
    assert_eq!(
        second.deleted,
        [format!("{PATH}t=1/_9.seg")],
        "only the unnamed one"
    );
    assert!(
        rig.emulator
            .head(&format!("{PATH}t=1/_0.seg"))
            .unwrap()
            .is_some()
    );

    assert!(rig.store.release(ID, &mut vec![0u8; 1 << 16]).unwrap());
    sweep(&rig.snapshot(), &mut memory);
    let after = sweep(&rig.snapshot(), &mut memory);
    assert_eq!(
        after.deleted,
        [format!("{PATH}t=1/_0.seg")],
        "free once released"
    );
}

#[test]
fn a_snapshot_answers_each_shard_once_so_a_view_made_after_it_cannot_lose_its_files() {
    let rig = Rig::new();
    let put = |rest: &str| {
        rig.emulator
            .put(
                &format!("{PATH}{rest}"),
                &mut SliceSource::new(b"bytes"),
                &mut [0u8; 64],
            )
            .unwrap();
    };
    put("t=1/_0.seg");
    put("t=2/_1.seg");
    let entry = ManifestEntry {
        name: "_1.cfs",
        location: FileLocation::Whole { term_dir: "t=2" }.encode(),
        length: 5,
    };
    let manifest = encode_manifest(2, Some("n1"), &[entry], -1, -1).unwrap();
    rig.emulator
        .create_register(&format!("{PATH}manifest"), &manifest)
        .unwrap();
    let mut memory = Memory::new();
    let sweep = |pins: &PointInTimePins, memory: &mut Memory| {
        let clock = || 1_000;
        Collector::new(&*rig.emulator, BASE, pins, &clock, LIVE)
            .sweep(SHARD, memory, &mut vec![0u8; 1 << 16])
            .unwrap()
    };
    let old = rig.snapshot();
    sweep(&old, &mut memory);
    // A view naming the watched blob is made after the snapshot.
    rig.create(&logs_view(ID, vec![file("_0.seg", "t=1")]));
    let stale = sweep(&old, &mut memory);
    assert!(
        stale.deleted.is_empty(),
        "the stale snapshot is not trusted again"
    );
    assert!(
        rig.emulator
            .head(&format!("{PATH}t=1/_0.seg"))
            .unwrap()
            .is_some()
    );
    let fresh = sweep(&rig.snapshot(), &mut memory);
    assert!(fresh.deleted.is_empty(), "a fresh one sees the view");
    assert!(
        rig.emulator
            .head(&format!("{PATH}t=1/_0.seg"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn an_expired_view_pins_nothing_and_a_kept_alive_one_still_does() {
    let rig = Rig::new();
    rig.create(&logs_view(ID, vec![file("_0.si", "t=1")]));
    rig.create(&logs_view(OTHER, vec![file("_1.si", "t=2")]));
    assert!(
        rig.store
            .extend(OTHER, 20_000, None, &mut vec![0u8; 1 << 16])
            .unwrap()
    );
    rig.now.store(10_000, Ordering::SeqCst);
    assert_eq!(keys(&rig.snapshot()), [format!("{PATH}t=2/_1.si")]);
    assert!(
        !rig.store
            .extend(ID, 30_000, None, &mut vec![0u8; 1 << 16])
            .unwrap(),
        "the expired one stays released"
    );
}

#[test]
fn a_capture_of_another_incarnation_of_the_index_still_holds_it() {
    let rig = Rig::new();
    let mut marker = view(ID, "logs", Some("u-other"), 0, Vec::new());
    marker.capturing = true;
    marker.shards.clear();
    rig.create(&marker);
    assert_eq!(rig.snapshot().pinned(&SHARD).unwrap(), Pinned::Everything);
}

#[test]
fn every_entry_of_a_shard_a_record_names_is_pinned() {
    let mut twice = logs_view(ID, vec![file("_0.si", "t=1")]);
    twice.shards.push(PitShard {
        shard: 0,
        term: 1,
        files: vec![file("_1.si", "t=2")],
    });
    let pins = PointInTimePins::of(BASE, vec![twice]);
    assert_eq!(
        keys(&pins),
        [format!("{PATH}t=1/_0.si"), format!("{PATH}t=2/_1.si")]
    );
}

#[test]
fn more_live_views_than_the_cap_fail_the_pass_closed() {
    let rig = Rig::new();
    rig.create(&logs_view(ID, vec![file("_0.si", "t=1")]));
    rig.create(&logs_view(OTHER, vec![file("_1.si", "t=2")]));
    let mut buf = vec![0u8; 1 << 16];
    assert!(PointInTimePins::snapshot(&rig.store, BASE, 1, &mut buf).is_err());
    assert!(PointInTimePins::snapshot(&rig.store, BASE, 2, &mut buf).is_ok());
}
