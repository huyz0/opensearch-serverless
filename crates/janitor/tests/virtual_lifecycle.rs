//! docs/specs/virtual-time-soak/SPEC.md AC-4 and AC-5: the garbage
//! collector's sightings and the points in time's deadlines, over seven
//! virtual days on a clock the test steps, on the emulator.

// ARITH: times and counts here are test constants and small sums; an
// overflow panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_format::{
    FileLocation, ManifestEntry, PitFile, PitShard, PointInTime, ShardRef, encode_manifest,
};
use oss_janitor::gc::{Collector, Config, Memory, Pins, PointInTimePins};
use oss_meta::PointInTimeStore;

const BASE: &str = "d/";
const PATH: &str = "d/segments/logs#u1#0/";
const SHARD: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 0,
};

const SWEEP: u64 = 5 * 60_000;
const HOUR: u64 = 60 * 60_000;
const WEEK: u64 = 7 * 24 * HOUR;
const START: u64 = 1_700_000_000_000;

/// A clock that moves only when the test steps it.
#[derive(Clone)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(START)))
    }
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn step(&self, millis: u64) {
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

fn put(store: &Emulator, rest: &str) {
    let mut buf = [0u8; 64];
    store
        .put(
            &format!("{PATH}{rest}"),
            &mut SliceSource::new(b"bytes"),
            &mut buf,
        )
        .expect("put");
}

/// A manifest of term 2 naming one blob of term 1, so every other blob of
/// term 1 is unreferenced and older than the live term.
fn publish_manifest(store: &Emulator) {
    let entries = [ManifestEntry {
        name: "_keep.cfs",
        location: FileLocation::Whole { term_dir: "t=1" }.encode(),
        length: 5,
    }];
    let value = encode_manifest(2, Some("n1"), &entries, -1, -1).expect("manifest");
    store
        .create_register(&format!("{PATH}manifest"), &value)
        .expect("manifest register");
}

fn exists(store: &Emulator, rest: &str) -> bool {
    store
        .head(&format!("{PATH}{rest}"))
        .expect("head")
        .is_some()
}

fn config(window: u64, dry_run: bool) -> Config {
    Config {
        dry_run,
        minimum_unreferenced_millis: i64::try_from(window).expect("window"),
        max_watched: 100_000,
        max_pinned: 100_000,
        relist_after_millis: 0,
    }
}

struct Nothing;

impl Pins for Nothing {
    fn pinned(
        &self,
        _: &ShardRef<'_>,
    ) -> Result<oss_janitor::gc::Pinned, oss_janitor::JanitorError> {
        Ok(oss_janitor::gc::Pinned::Blobs(Vec::new()))
    }
}

/// Seven virtual days of sweeps every five minutes, a new unreferenced blob
/// of an older term every hour: a blob goes only after it has been seen
/// unreferenced for the whole window, and goes at the first sweep after it.
fn sightings(window: u64, dry_run: bool) -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let store = Emulator::new(64 << 20, 1);
    let clock = Clock::new();
    publish_manifest(&store);
    put(&store, "t=1/_keep.cfs");
    let mut memory = Memory::new();
    let mut appeared = BTreeMap::new();
    let mut deleted = BTreeMap::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut hour = 0u64;
    while clock.now() - START <= WEEK + 2 * HOUR + window {
        let elapsed = clock.now() - START;
        if elapsed.is_multiple_of(HOUR) && elapsed < WEEK {
            let name = format!("t=1/b{hour}.seg");
            put(&store, &name);
            appeared.insert(name, clock.now());
            hour += 1;
        }
        let now = i64::try_from(clock.now()).expect("now");
        let read_clock = move || now;
        let collector =
            Collector::new(&store, BASE, &Nothing, &read_clock, config(window, dry_run));
        let sweep = collector
            .sweep(SHARD, &mut memory, &mut buf)
            .expect("sweep");
        for key in sweep.deleted {
            let rest = key.strip_prefix(PATH).expect("under the shard").to_owned();
            deleted.insert(rest, clock.now());
        }
        clock.step(SWEEP);
    }
    assert!(
        exists(&store, "t=1/_keep.cfs"),
        "a blob the live manifest names was deleted"
    );
    (appeared, deleted)
}

#[test]
fn a_blob_goes_only_after_its_whole_window_and_every_one_goes_in_a_week() {
    let window = HOUR;
    let (appeared, deleted) = sightings(window, false);
    assert_eq!(appeared.len(), 168);
    for (name, born) in &appeared {
        let gone = deleted
            .get(name)
            .unwrap_or_else(|| panic!("{name} was never deleted"));
        assert!(
            gone - born >= window,
            "{name} went after {} ms, before its {window} ms window",
            gone - born
        );
        assert!(
            gone - born <= window + 2 * SWEEP,
            "{name} outlived its window by {} ms",
            gone - born - window
        );
    }
    assert_eq!(deleted.len(), appeared.len(), "something else was deleted");
}

#[test]
fn a_dry_run_over_the_week_deletes_nothing() {
    let (appeared, deleted) = sightings(HOUR, true);
    assert_eq!(appeared.len(), 168);
    assert!(deleted.is_empty());
}

fn id(n: u64) -> String {
    format!("{}{n:02}", "A".repeat(20))
}

fn view(n: u64, expires: u64, file: &str) -> PointInTime {
    PointInTime {
        id: id(n),
        index: "logs".to_owned(),
        index_uuid: Some("u1".to_owned()),
        expires_at_millis: expires,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 1,
            files: vec![PitFile {
                name: file.to_owned(),
                location: "t=1".to_owned(),
                length: Some(5),
            }],
        }],
    }
}

/// Views of keep-alives from an hour to three days, one of them extended
/// before it ends and one after: each is honoured to the millisecond of its
/// deadline and not past it, and the blob it pins goes only after it.
#[test]
fn a_point_in_time_lives_to_its_deadline_and_pins_its_blobs_until_then() {
    let emulator = Arc::new(Emulator::new(64 << 20, 1));
    let clock = Clock::new();
    let reader = clock.clone();
    let views = PointInTimeStore::new(
        Arc::clone(&emulator) as Arc<dyn BlobStore>,
        BASE,
        Arc::new(move || reader.now()),
    )
    .expect("store");
    publish_manifest(&emulator);
    let mut buf = vec![0u8; 1 << 16];
    // (keep-alive, extended by, extended at)
    let plan: [(u64, Option<(u64, u64)>); 6] = [
        (HOUR, None),
        (3 * HOUR, None),
        (12 * HOUR, Some((6 * HOUR, 10 * HOUR))),
        (24 * HOUR, None),
        (48 * HOUR, Some((24 * HOUR, 72 * HOUR))),
        (72 * HOUR, None),
    ];
    let mut deadline: Vec<u64> = Vec::new();
    for (n, (alive, _)) in plan.iter().enumerate() {
        let file = format!("_{n}.si");
        put(&emulator, &format!("t=1/{file}"));
        let expires = START + alive;
        views
            .create(&view(n as u64, expires, &file), &mut buf)
            .expect("create");
        deadline.push(expires);
    }
    let mut memory = Memory::new();
    let mut late_extend_refused = false;
    while clock.now() - START <= WEEK {
        let now = clock.now();
        let elapsed = now - START;
        for (n, (_, extension)) in plan.iter().enumerate() {
            if let Some((by, at)) = extension
                && elapsed == *at
                && now < deadline[n]
            {
                assert!(
                    views
                        .extend(&id(n as u64), deadline[n] + by, None, &mut buf)
                        .expect("extend")
                );
                deadline[n] += by;
            }
        }
        // The one that is extended after it ended is refused.
        if elapsed == 5 * HOUR {
            assert!(
                !views
                    .extend(&id(0), now + 100 * HOUR, None, &mut buf)
                    .expect("late extend"),
                "an expired view was extended"
            );
            late_extend_refused = true;
        }
        let live: Vec<String> = views
            .live(&mut buf)
            .expect("live")
            .into_iter()
            .map(|view| view.id)
            .collect();
        for (n, end) in deadline.iter().enumerate() {
            let there = live.contains(&id(n as u64));
            assert_eq!(
                there,
                now < *end,
                "view {n} at +{elapsed} ms of a deadline at +{}",
                end - START
            );
        }
        let pins = PointInTimePins::snapshot(&views, BASE, 1000, &mut buf).expect("pins");
        let clock_ms = i64::try_from(now).expect("now");
        let read_clock = move || clock_ms;
        let collector = Collector::new(&*emulator, BASE, &pins, &read_clock, config(0, false));
        collector
            .sweep(SHARD, &mut memory, &mut buf)
            .expect("sweep");
        for (n, end) in deadline.iter().enumerate() {
            let held = exists(&emulator, &format!("t=1/_{n}.si"));
            if now < *end {
                assert!(held, "view {n}'s blob was deleted while it was live");
            }
        }
        clock.step(SWEEP);
    }
    assert!(late_extend_refused);
    // Past every deadline the blobs are collectable and go (two sightings).
    for n in 0..plan.len() {
        assert!(
            !exists(&emulator, &format!("t=1/_{n}.si")),
            "view {n}'s blob outlived its view"
        );
    }
}
