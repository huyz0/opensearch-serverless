//! docs/specs/janitor-gc/SPEC.md: the garbage collector, on the emulator
//! with its meter and a clock the test moves.

// ARITH: times and counts here are test constants and small sums; an
// overflow panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::Cell;
use std::sync::Mutex;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{FileLocation, ManifestEntry, ShardRef, encode_manifest};
use oss_janitor::JanitorError;
use oss_janitor::gc::{Collector, Config, Memory, Outcome, Pinned, Pins};

const BASE: &str = "d/";
const SHARD: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 0,
};
const PATH: &str = "d/segments/logs#u1#0/";

struct Fixed(Mutex<Pinned>);

impl Fixed {
    fn none() -> Self {
        Self(Mutex::new(Pinned::Blobs(Vec::new())))
    }
    fn set(&self, pinned: Pinned) {
        *self.0.lock().unwrap() = pinned;
    }
}

impl Pins for Fixed {
    fn pinned(&self, _: &ShardRef<'_>) -> Result<Pinned, JanitorError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

struct Rig {
    store: Emulator,
    pins: Fixed,
    now: Cell<i64>,
    memory: Memory,
}

impl Rig {
    fn new() -> Self {
        Self {
            store: Emulator::new(64 << 20, 1),
            pins: Fixed::none(),
            now: Cell::new(1_000),
            memory: Memory::new(),
        }
    }

    fn blob(&self, rest: &str) {
        let key = format!("{PATH}{rest}");
        let mut buf = [0u8; 64];
        self.store
            .put(&key, &mut SliceSource::new(b"bytes"), &mut buf)
            .unwrap();
    }

    /// A manifest of `term` naming `files` as `(file, location)`.
    fn manifest(&self, term: u64, files: &[(&str, FileLocation<'_>)]) {
        let entries: Vec<ManifestEntry<'_>> = files
            .iter()
            .map(|(name, location)| ManifestEntry {
                name,
                location: location.encode(),
                length: 5,
            })
            .collect();
        let value = encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap();
        let key = format!("{PATH}manifest");
        self.store.create_register(&key, &value).unwrap();
    }

    fn sweep(&mut self, config: Config) -> oss_janitor::gc::Sweep {
        self.try_sweep(config).unwrap()
    }

    fn try_sweep(&mut self, config: Config) -> Result<oss_janitor::gc::Sweep, JanitorError> {
        let now = self.now.get();
        let clock = move || now;
        let collector = Collector::new(&self.store, BASE, &self.pins, &clock, config);
        let mut buf = vec![0u8; 1 << 16];
        collector.sweep(SHARD, &mut self.memory, &mut buf)
    }

    fn exists(&self, rest: &str) -> bool {
        self.store.head(&format!("{PATH}{rest}")).unwrap().is_some()
    }

    fn count(&self, op: S3Op) -> u64 {
        self.store.meter().count(op)
    }
}

const LIVE: Config = Config {
    dry_run: false,
    minimum_unreferenced_millis: 0,
    max_watched: 100_000,
    max_pinned: 100_000,
};

fn whole(term_dir: &str) -> FileLocation<'_> {
    FileLocation::Whole { term_dir }
}

#[test]
fn an_unreferenced_blob_of_an_older_term_goes_on_the_second_sweep_not_the_first() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.blob("t=2/_1.seg");
    rig.manifest(2, &[("_1.cfs", whole("t=2"))]);
    let first = rig.sweep(LIVE);
    assert_eq!(first.outcome, Outcome::Swept);
    assert!(first.deleted.is_empty());
    assert_eq!(first.watching, 1);
    assert!(rig.exists("t=1/_0.seg"));
    let second = rig.sweep(LIVE);
    assert_eq!(second.deleted, [format!("{PATH}t=1/_0.seg")]);
    assert!(!rig.exists("t=1/_0.seg"));
    assert!(rig.exists("t=2/_1.seg"));
    assert_eq!(second.watching, 0);
    assert_eq!(rig.memory.shards(), 0);
}

#[test]
fn a_blob_the_manifest_names_is_never_deleted_whatever_its_term() {
    let mut rig = Rig::new();
    // inherited from term 1: one whole file, one range of a pack
    rig.blob("t=1/_0.cfs");
    rig.blob("t=1/_2.seg");
    rig.blob("t=1/_9.seg");
    rig.blob("t=2/_1.seg");
    let packed = FileLocation::Packed {
        term_dir: "t=1",
        blob: "_2.seg",
        offset: 0,
        length: 5,
    };
    rig.manifest(2, &[("_0.cfs", whole("t=1")), ("_2.cfs", packed)]);
    rig.sweep(LIVE);
    let gone = rig.sweep(LIVE);
    assert_eq!(gone.deleted, [format!("{PATH}t=1/_9.seg")]);
    for _ in 0..3 {
        assert!(rig.sweep(LIVE).deleted.is_empty());
    }
    assert!(rig.exists("t=1/_0.cfs"));
    assert!(rig.exists("t=1/_2.seg"));
}

#[test]
fn a_blob_of_the_live_or_a_later_term_is_never_deleted() {
    let mut rig = Rig::new();
    rig.blob("t=2/_5.seg");
    rig.blob("t=3/_6.seg");
    rig.manifest(2, &[("_1.cfs", whole("t=2"))]);
    for _ in 0..4 {
        rig.sweep(LIVE);
    }
    assert!(rig.exists("t=2/_5.seg"));
    assert!(rig.exists("t=3/_6.seg"));
}

#[test]
fn a_pinned_blob_stays_and_leaves_the_memory() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    assert_eq!(rig.sweep(LIVE).watching, 1);
    rig.pins
        .set(Pinned::Blobs(vec![format!("{PATH}t=1/_0.seg")]));
    let pinned = rig.sweep(LIVE);
    assert!(pinned.deleted.is_empty());
    assert_eq!(pinned.watching, 0);
    // unpinned: its clock starts again, so it needs two more sweeps
    rig.pins.set(Pinned::Blobs(Vec::new()));
    assert!(rig.sweep(LIVE).deleted.is_empty());
    assert_eq!(rig.sweep(LIVE).deleted.len(), 1);
}

#[test]
fn pins_that_cannot_say_delete_nothing_and_keep_the_memory() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    assert_eq!(rig.sweep(LIVE).watching, 1);
    rig.pins.set(Pinned::Everything);
    let blind = rig.sweep(LIVE);
    assert_eq!(blind.outcome, Outcome::Pinned);
    assert!(blind.deleted.is_empty());
    assert_eq!(rig.memory.watching(PATH), 1);
    assert!(rig.exists("t=1/_0.seg"));
    // the grace was not restarted
    rig.pins.set(Pinned::Blobs(Vec::new()));
    assert_eq!(rig.sweep(LIVE).deleted.len(), 1);
}

#[test]
fn a_shard_without_a_manifest_is_not_listed_and_a_bad_one_deletes_nothing() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    let none = rig.sweep(LIVE);
    assert_eq!(none.outcome, Outcome::NoCommit);
    assert!(none.deleted.is_empty());
    assert_eq!(rig.count(S3Op::List), 0);
    let key = format!("{PATH}manifest");
    rig.store.create_register(&key, b"{not json").unwrap();
    assert!(rig.try_sweep(LIVE).is_err());
    assert!(rig.try_sweep(LIVE).is_err());
    assert!(rig.exists("t=1/_0.seg"));
}

#[test]
fn a_minimum_age_holds_a_blob_seen_twice_until_the_clock_has_moved() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    let config = Config {
        dry_run: false,
        minimum_unreferenced_millis: 1_000,
        max_watched: 100_000,
        max_pinned: 100_000,
    };
    rig.sweep(config);
    rig.now.set(1_500);
    assert!(rig.sweep(config).deleted.is_empty());
    rig.now.set(1_999);
    assert!(rig.sweep(config).deleted.is_empty());
    rig.now.set(2_000);
    assert_eq!(rig.sweep(config).deleted.len(), 1);
}

#[test]
fn a_new_owner_with_an_empty_memory_deletes_nothing_on_its_first_sweep() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    rig.sweep(LIVE);
    rig.memory = Memory::new();
    assert!(rig.sweep(LIVE).deleted.is_empty());
    assert!(rig.exists("t=1/_0.seg"));
    assert_eq!(rig.sweep(LIVE).deleted.len(), 1);
}

#[test]
fn a_dry_run_deletes_nothing_and_reports_what_it_would() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    let dry = Config {
        dry_run: true,
        minimum_unreferenced_millis: 0,
        max_watched: 100_000,
        max_pinned: 100_000,
    };
    assert!(rig.sweep(dry).would_delete.is_empty());
    let second = rig.sweep(dry);
    assert_eq!(second.would_delete, [format!("{PATH}t=1/_0.seg")]);
    assert!(second.deleted.is_empty());
    assert!(rig.exists("t=1/_0.seg"));
    assert_eq!(rig.count(S3Op::Delete), 0);
    // still watched, so flipping the switch deletes at the next sweep
    assert_eq!(rig.sweep(dry).would_delete.len(), 1);
    assert_eq!(rig.sweep(LIVE).deleted.len(), 1);
}

#[test]
fn keys_that_are_not_term_blobs_are_never_judged() {
    let mut rig = Rig::new();
    rig.blob("t=x/_0.seg");
    rig.blob("t=1/nested/_0.seg");
    rig.blob("other");
    rig.manifest(5, &[]);
    for _ in 0..3 {
        assert!(rig.sweep(LIVE).deleted.is_empty());
    }
    assert!(rig.exists("t=x/_0.seg"));
    assert!(rig.exists("t=1/nested/_0.seg"));
    assert!(rig.exists("other"));
    assert!(
        rig.store
            .head(&format!("{PATH}manifest"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_sweep_is_one_register_read_a_listing_and_a_delete_batch() {
    let mut rig = Rig::new();
    for i in 0..1_100 {
        rig.blob(&format!("t=1/_{i}.seg"));
    }
    rig.manifest(2, &[]);
    rig.sweep(LIVE);
    assert_eq!(rig.count(S3Op::List), 2);
    let before = rig.store.meter().total();
    let reads = rig.count(S3Op::Get) + rig.count(S3Op::GetRange);
    let second = rig.sweep(LIVE);
    assert_eq!(second.deleted.len(), 1_100);
    assert_eq!(rig.count(S3Op::List), 4);
    assert_eq!(rig.count(S3Op::Delete), 2);
    assert_eq!(rig.count(S3Op::Head), 0);
    // the register read is the only GET of a sweep
    assert_eq!(rig.count(S3Op::Get) + rig.count(S3Op::GetRange) - reads, 1);
    assert_eq!(rig.store.meter().total() - before, 1 + 2 + 2);
}

#[test]
fn the_memory_holds_only_what_is_still_a_candidate() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.blob("t=1/_1.seg");
    rig.manifest(2, &[]);
    rig.sweep(LIVE);
    assert_eq!(rig.memory.watching(PATH), 2);
    assert_eq!(rig.memory.shards(), 1);
    rig.sweep(LIVE);
    assert_eq!(rig.memory.watching(PATH), 0);
    assert_eq!(rig.memory.shards(), 0);
}

#[test]
fn a_sweep_watches_no_more_than_its_cap_and_later_sweeps_take_the_rest() {
    let mut rig = Rig::new();
    for i in 0..5 {
        rig.blob(&format!("t=1/_{i}.seg"));
    }
    rig.manifest(2, &[]);
    let capped = Config {
        max_watched: 2,
        ..LIVE
    };
    assert_eq!(rig.sweep(capped).watching, 2);
    assert_eq!(rig.memory.watching(PATH), 2);
    // the two watched go; the next sweep starts on two more
    assert_eq!(rig.sweep(capped).deleted.len(), 2);
    rig.sweep(capped);
    assert_eq!(rig.sweep(capped).deleted.len(), 2);
    rig.sweep(capped);
    assert_eq!(rig.sweep(capped).deleted.len(), 1);
    assert!(!rig.exists("t=1/_4.seg"));
}

#[test]
fn a_lowered_cap_shrinks_the_memory_but_never_starves_an_aged_blob() {
    let mut rig = Rig::new();
    for i in 0..4 {
        rig.blob(&format!("t=1/_{i}.seg"));
    }
    rig.manifest(2, &[]);
    assert_eq!(rig.sweep(LIVE).watching, 4);
    let capped = Config {
        max_watched: 1,
        ..LIVE
    };
    // all four are due, so all four go though the cap is one
    assert_eq!(rig.sweep(capped).deleted.len(), 4);
}

#[test]
fn pins_naming_more_keys_than_the_cap_are_as_if_they_said_everything() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    rig.sweep(LIVE);
    rig.pins.set(Pinned::Blobs(vec!["a".into(), "b".into()]));
    let capped = Config {
        max_pinned: 1,
        ..LIVE
    };
    let sweep = rig.sweep(capped);
    assert_eq!(sweep.outcome, Outcome::Pinned);
    assert!(rig.exists("t=1/_0.seg"));
    // exactly the cap is honoured as the keys it names
    rig.pins
        .set(Pinned::Blobs(vec![format!("{PATH}t=1/_0.seg")]));
    assert_eq!(rig.sweep(capped).outcome, Outcome::Swept);
}

#[test]
fn a_failed_delete_deletes_nothing_it_claims_and_leaves_the_memory() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    rig.sweep(LIVE);
    rig.store.fail_next(S3Op::Delete, 8);
    assert!(rig.try_sweep(LIVE).is_err());
    assert_eq!(rig.memory.watching(PATH), 1);
    assert!(rig.exists("t=1/_0.seg"));
    // the store recovers and the next sweep finishes it
    rig.store.fail_next(S3Op::Delete, 0);
    assert_eq!(rig.sweep(LIVE).deleted.len(), 1);
}
