//! docs/specs/janitor-sweep-scope/SPEC.md: judging older terms without
//! listing them, on the emulator with its meter and a clock the test moves.

// ARITH: times and counts here are test constants and small sums.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::Cell;
use std::sync::Mutex;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{FileLocation, ManifestEntry, ShardRef, encode_manifest};
use oss_janitor::JanitorError;
use oss_janitor::gc::{Collector, Config, Memory, Pinned, Pins};

const BASE: &str = "d/";
const SHARD: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 0,
};
const PATH: &str = "d/segments/logs#u1#0/";
const HOUR: i64 = 3_600_000;

struct Fixed(Mutex<Result<Pinned, ()>>);

impl Pins for Fixed {
    fn pinned(&self, _: &ShardRef<'_>) -> Result<Pinned, JanitorError> {
        match &*self.0.lock().unwrap() {
            Ok(p) => Ok(p.clone()),
            Err(()) => Err(JanitorError::Pins("unreadable")),
        }
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
            pins: Fixed(Mutex::new(Ok(Pinned::Blobs(Vec::new())))),
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

    fn manifest(&self, term: u64, files: &[(&str, &str)]) {
        let entries: Vec<ManifestEntry<'_>> = files
            .iter()
            .map(|(name, term_dir)| ManifestEntry {
                name,
                location: FileLocation::Whole { term_dir }.encode(),
                length: 5,
            })
            .collect();
        let value = encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap();
        let key = format!("{PATH}manifest");
        self.store.delete(&key).ok();
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

    fn lists(&self) -> u64 {
        self.store.meter().count(S3Op::List)
    }
}

const fn config(relist_after_millis: i64, dry_run: bool) -> Config {
    Config {
        dry_run,
        minimum_unreferenced_millis: 0,
        max_watched: 100_000,
        max_pinned: 100_000,
        relist_after_millis,
    }
}

#[test]
fn with_no_interval_every_sweep_lists() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    assert!(rig.sweep(config(0, false)).listed);
    assert!(rig.sweep(config(0, false)).listed);
    assert_eq!(rig.lists(), 2);
}

#[test]
fn a_sweep_inside_the_interval_lists_nothing_and_still_collects() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.blob("t=1/_1.seg");
    rig.manifest(2, &[("_1.cfs", "t=1")]);
    let first = rig.sweep(config(HOUR, false));
    assert!(first.listed);
    assert_eq!(first.watching, 2);
    // _1.seg is named by nothing (the manifest names _1.cfs), _0.seg neither
    rig.now.set(2_000);
    let second = rig.sweep(config(HOUR, false));
    assert!(!second.listed);
    assert_eq!(rig.lists(), 1);
    let mut deleted = second.deleted.clone();
    deleted.sort();
    assert_eq!(
        deleted,
        [format!("{PATH}t=1/_0.seg"), format!("{PATH}t=1/_1.seg")]
    );
    assert!(!rig.exists("t=1/_0.seg"));
}

#[test]
fn an_unlisted_sweep_never_deletes_what_the_manifest_or_a_pin_names() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.blob("t=1/_1.seg");
    rig.blob("t=1/_2.seg");
    rig.manifest(2, &[("_0.seg", "t=1")]);
    rig.sweep(config(HOUR, false));
    // a pin now names _2.seg, which the first sweep put on watch
    *rig.pins.0.lock().unwrap() = Ok(Pinned::Blobs(vec![format!("{PATH}t=1/_2.seg")]));
    rig.now.set(2_000);
    let second = rig.sweep(config(HOUR, false));
    assert!(!second.listed);
    assert_eq!(second.deleted, [format!("{PATH}t=1/_1.seg")]);
    assert!(rig.exists("t=1/_0.seg"));
    assert!(rig.exists("t=1/_2.seg"));
    // the pin lets go: the blob it held is judged again, from the scope
    *rig.pins.0.lock().unwrap() = Ok(Pinned::Blobs(Vec::new()));
    rig.now.set(3_000);
    rig.sweep(config(HOUR, false));
    rig.now.set(4_000);
    let last = rig.sweep(config(HOUR, false));
    assert!(!last.listed);
    assert_eq!(last.deleted, [format!("{PATH}t=1/_2.seg")]);
    assert!(rig.exists("t=1/_0.seg"));
}

#[test]
fn a_new_term_or_the_interval_lists_again_and_finds_the_zombies_blob() {
    let mut rig = Rig::new();
    rig.manifest(2, &[]);
    assert!(rig.sweep(config(HOUR, false)).listed);
    // a fenced zombie writes into the older term after the listing
    rig.blob("t=1/_9.seg");
    rig.now.set(2_000);
    assert!(!rig.sweep(config(HOUR, false)).listed);
    assert!(rig.exists("t=1/_9.seg"));
    // the interval passes: listed, found, watched; the next sweep deletes it
    rig.now.set(2_000 + HOUR);
    let found = rig.sweep(config(HOUR, false));
    assert!(found.listed);
    assert_eq!(found.watching, 1);
    rig.now.set(3_000 + HOUR);
    let gone = rig.sweep(config(HOUR, false));
    assert!(!gone.listed);
    assert_eq!(gone.deleted, [format!("{PATH}t=1/_9.seg")]);
    // a later term lists at once
    rig.manifest(3, &[]);
    rig.now.set(4_000 + HOUR);
    assert!(rig.sweep(config(HOUR, false)).listed);
}

#[test]
fn a_dry_run_unlisted_reports_what_a_listing_would() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    let listed = rig.sweep(config(HOUR, true));
    assert!(listed.listed);
    rig.now.set(2_000);
    let unlisted = rig.sweep(config(HOUR, true));
    assert!(!unlisted.listed);
    assert_eq!(unlisted.would_delete, [format!("{PATH}t=1/_0.seg")]);
    assert!(unlisted.deleted.is_empty());
    assert!(rig.exists("t=1/_0.seg"));
}

#[test]
fn a_failed_sweep_leaves_the_scope() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    rig.sweep(config(HOUR, false));
    *rig.pins.0.lock().unwrap() = Err(());
    rig.now.set(2_000);
    assert!(rig.try_sweep(config(HOUR, false)).is_err());
    *rig.pins.0.lock().unwrap() = Ok(Pinned::Blobs(Vec::new()));
    rig.now.set(3_000);
    let again = rig.sweep(config(HOUR, false));
    assert!(!again.listed);
    assert_eq!(again.deleted, [format!("{PATH}t=1/_0.seg")]);
}

#[test]
fn the_interval_is_exact_and_a_clock_that_stands_still_does_not_list() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    assert!(rig.sweep(config(HOUR, false)).listed);
    // the same instant: still inside the interval
    assert!(!rig.sweep(config(HOUR, true)).listed);
    // one millisecond short of the interval: not listed; at it: listed
    rig.now.set(1_000 + HOUR - 1);
    assert!(!rig.sweep(config(HOUR, true)).listed);
    rig.now.set(1_000 + HOUR);
    assert!(rig.sweep(config(HOUR, true)).listed);
    // a clock that went back lists too
    rig.now.set(500);
    assert!(rig.sweep(config(HOUR, true)).listed);
}

#[test]
fn retain_keeps_what_it_accepts_and_drops_the_rest() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    rig.sweep(config(HOUR, false));
    assert_eq!((rig.memory.shards(), rig.memory.scopes()), (1, 1));
    rig.memory.retain(|path| path == PATH);
    assert_eq!((rig.memory.shards(), rig.memory.scopes()), (1, 1));
    rig.memory.retain(|_| false);
    assert_eq!((rig.memory.shards(), rig.memory.scopes()), (0, 0));
}

#[test]
fn an_evicted_shard_is_swept_as_a_new_owners() {
    let mut rig = Rig::new();
    rig.blob("t=1/_0.seg");
    rig.manifest(2, &[]);
    rig.sweep(config(HOUR, false));
    rig.memory.retain(|_| false);
    rig.now.set(2_000);
    let first = rig.sweep(config(HOUR, false));
    assert!(first.listed, "no scope: listed");
    assert!(first.deleted.is_empty(), "first sight deletes nothing");
    assert!(rig.exists("t=1/_0.seg"));
    assert_eq!(first.watching, 1);
}
