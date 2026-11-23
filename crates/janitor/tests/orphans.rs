//! docs/specs/janitor-orphan-sweep/SPEC.md: the resumable sweep of shard
//! containers whose index is gone.

// ARITH: counts here are test constants and small sums.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{IndexRecord, PitShard, PointInTime, descriptor_key};
use oss_janitor::JanitorError;
use oss_janitor::orphans::{
    Config, IndexPins, PinSource, PointInTimeSource, SnapshotSource, Sweeper,
};
use oss_meta::{DeleteMode, DescriptorStore, PointInTimeStore};
use oss_testkit::budget;

const BASE: &str = "d/";
const U1: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const U2: &str = "BBBBBBBBBBBBBBBBBBBBBB";
const U3: &str = "CCCCCCCCCCCCCCCCCCCCCC";
const ID: &str = "PPPPPPPPPPPPPPPPPPPPPP";
const CURSOR: &str = "d/gc/orphan-sweep-cursor";
const LIVE: Config = Config {
    dry_run: false,
    max_pins: 1000,
    descriptor_bytes: 1 << 16,
};

struct Rig {
    emulator: Arc<Emulator>,
    descriptors: DescriptorStore,
    pits: PointInTimeStore,
}

impl Rig {
    fn new() -> Self {
        let emulator = Arc::new(Emulator::new(64 << 20, 1));
        let store = Arc::clone(&emulator) as Arc<dyn BlobStore>;
        let clock = Arc::new(AtomicU64::new(1_000));
        Self {
            descriptors: DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional)
                .unwrap(),
            pits: PointInTimeStore::new(
                store,
                BASE,
                Arc::new(move || clock.load(Ordering::SeqCst)),
            )
            .unwrap(),
            emulator,
        }
    }

    fn store(&self) -> Arc<dyn BlobStore> {
        Arc::clone(&self.emulator) as Arc<dyn BlobStore>
    }

    fn put(&self, key: &str, body: &[u8]) {
        let mut buf = [0u8; 64];
        self.emulator
            .put(key, &mut SliceSource::new(body), &mut buf)
            .unwrap();
    }

    /// A container with two blobs and a `manifest` register.
    fn container(&self, name: &str) {
        self.put(&format!("d/segments/{name}/a"), b"x");
        self.put(&format!("d/segments/{name}/b"), b"y");
        self.emulator
            .create_register(&format!("d/segments/{name}/manifest"), b"m")
            .unwrap();
    }

    fn live(&self, name: &str, uuid: &str) {
        self.descriptors
            .create(&IndexRecord::new(name, uuid, 1), 1, &mut vec![0u8; 1 << 16])
            .unwrap();
    }

    fn exists(&self, name: &str) -> bool {
        let mut any = false;
        // LIST: the test's own look at what is left.
        self.emulator
            .list_prefix(&format!("d/segments/{name}/"), None, 10, &mut |_, _| {
                any = true;
            })
            .unwrap();
        any
    }

    fn cursor(&self) -> Option<String> {
        let mut buf = [0u8; 256];
        self.emulator
            .read_register(CURSOR, &mut buf)
            .unwrap()
            .map(|r| String::from_utf8(buf[..r.len].to_vec()).unwrap())
    }

    fn pit(&self, index: &str, uuid: Option<&str>, capturing: bool) {
        let pit = PointInTime {
            id: ID.to_owned(),
            index: index.to_owned(),
            index_uuid: uuid.map(str::to_owned),
            expires_at_millis: 1_000_000,
            capturing,
            shards: if capturing {
                Vec::new()
            } else {
                vec![PitShard {
                    shard: 0,
                    term: 1,
                    files: Vec::new(),
                }]
            },
        };
        self.pits.create(&pit, &mut vec![0u8; 1 << 16]).unwrap();
    }

    fn snapshot(&self, key: &str, index: &str, uuid: &str, shards: bool) {
        let tail = if shards {
            r#","shards":[{"shard":0,"term":1,"files":{}}]"#
        } else {
            ""
        };
        let body = format!(
            r#"{{"repo":"r","name":"s","uuid":"x","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{{"{index}":{{"uuid":"{uuid}","number_of_shards":1{tail}}}}}}}"#
        );
        self.put(&format!("d/snapshots/{key}"), body.as_bytes());
    }

    fn count(&self, op: S3Op) -> u64 {
        self.emulator.meter().count(op)
    }

    fn sources(&self) -> (PointInTimeSource<'_>, SnapshotSource) {
        (
            PointInTimeSource::new(&self.pits, 100),
            SnapshotSource::new(self.store(), BASE, 100, 1 << 16),
        )
    }

    fn sweeper<'a>(
        &'a self,
        config: Config,
        pit: &'a dyn PinSource,
        snapshots: &'a dyn PinSource,
    ) -> Sweeper<'a> {
        Sweeper::new(
            self.emulator.as_ref(),
            &self.descriptors,
            BASE,
            config,
            pit,
            snapshots,
        )
    }
}

fn name(index: &str, uuid: &str, shard: u32) -> String {
    format!("{index}#{uuid}#{shard}")
}

// AC-2
#[test]
fn a_container_of_a_gone_index_or_another_uuid_is_deleted_and_a_live_one_is_not() {
    let rig = Rig::new();
    rig.live("logs", U1);
    rig.live("moved", U2);
    rig.container(&name("logs", U1, 0));
    rig.container(&name("gone", U3, 0));
    rig.container(&name("moved", U3, 0));
    let (pit, snap) = rig.sources();
    let done = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap();
    assert_eq!((done.containers, done.candidates, done.orphans), (3, 2, 2));
    // Two blobs and the register each.
    assert_eq!(done.blobs, 6);
    assert!(rig.exists(&name("logs", U1, 0)));
    assert!(!rig.exists(&name("gone", U3, 0)));
    assert!(!rig.exists(&name("moved", U3, 0)));
    assert!(done.cycle_ended);
    assert_eq!(rig.cursor().as_deref(), Some(""));
}

// AC-3, AC-10
#[test]
fn a_container_whose_name_this_system_did_not_write_is_left() {
    let rig = Rig::new();
    let odd = [
        "junk".to_owned(),
        name("gone", "short", 0),
        name("gone", U3, 1),
        format!("gone#{U3}#01"),
        format!("gone#{U3}#4096"),
        format!("gone#{U3}#+1"),
        format!("gone#{U3}#0#9"),
        format!("UPPER#{U3}#0"),
    ];
    // `name("gone", U3, 1)` is well formed: take it out of the odd ones.
    for n in &odd[..] {
        rig.container(n);
    }
    let (pit, snap) = rig.sources();
    let done = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap();
    assert_eq!(done.orphans, 1, "only the well-formed one");
    assert_eq!(done.left_unparsed, odd.len() - 1);
    for n in odd.iter().filter(|n| **n != name("gone", U3, 1)) {
        assert!(rig.exists(n), "{n} must stay");
    }
}

// AC-1
#[test]
fn a_budget_of_zero_is_refused() {
    let rig = Rig::new();
    let (pit, snap) = rig.sources();
    let err = rig.sweeper(LIVE, &pit, &snap).slice(0).unwrap_err();
    assert!(matches!(err, JanitorError::Refused(_)));
    assert_eq!(rig.emulator.meter().total(), 0);
}

// AC-4
#[test]
fn a_descriptor_that_cannot_be_read_fails_the_slice_and_deletes_nothing() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    rig.emulator
        .create_register(&descriptor_key(BASE, "gone").unwrap(), b"{not json")
        .unwrap();
    let (pit, snap) = rig.sources();
    let err = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap_err();
    assert!(matches!(err, JanitorError::Meta(_)), "{err}");
    assert!(rig.exists(&name("gone", U3, 0)));
    assert_eq!(rig.cursor(), None);
}

// AC-5
#[test]
fn a_point_in_time_or_a_snapshot_or_a_capture_pins_its_index() {
    for pin in 0..4 {
        let rig = Rig::new();
        rig.container(&name("gone", U3, 0));
        match pin {
            0 => rig.pit("gone", Some(U3), false),
            1 => rig.pit("gone", None, true),
            2 => rig.snapshot("v2-h#h#g", "gone", U3, true),
            _ => rig.snapshot("r#s", "gone", U3, false),
        }
        let (pit, snap) = rig.sources();
        let done = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap();
        assert_eq!((done.pinned, done.orphans), (1, 0), "pin {pin}");
        assert!(rig.exists(&name("gone", U3, 0)));
    }
    // Another incarnation's pin does not hold this one.
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    rig.pit("gone", Some(U1), false);
    rig.snapshot("r#s", "gone", U2, true);
    let (pit, snap) = rig.sources();
    let done = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap();
    assert_eq!(done.orphans, 1);
}

// AC-5
#[test]
fn pins_that_cannot_be_read_or_are_too_many_stop_the_slice_with_nothing_deleted() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    rig.put("d/snapshots/bad", b"{not a snapshot");
    let (pit, snap) = rig.sources();
    let err = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap_err();
    assert!(matches!(err, JanitorError::Pins(_)), "{err}");
    assert!(rig.exists(&name("gone", U3, 0)));
    assert_eq!(rig.cursor(), None, "the cursor did not move");

    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    rig.snapshot("r#a", "one", U1, true);
    rig.snapshot("r#b", "two", U2, true);
    let (pit, snap) = rig.sources();
    let tight = Config {
        max_pins: 1,
        ..LIVE
    };
    assert!(rig.sweeper(tight, &pit, &snap).slice(100).is_err());
    let few = SnapshotSource::new(rig.store(), BASE, 1, 1 << 16);
    assert!(rig.sweeper(LIVE, &pit, &few).slice(100).is_err());
    assert!(rig.exists(&name("gone", U3, 0)));
}

/// A source that registers a point in time just before the real read.
struct Late<'a>(&'a Rig, PointInTimeSource<'a>);

impl PinSource for Late<'_> {
    fn collect(&self, into: &mut IndexPins) -> Result<(), JanitorError> {
        self.0.pit("gone", None, true);
        self.1.collect(into)
    }
}

// AC-5c
#[test]
fn a_pin_registered_after_the_descriptors_and_before_the_pins_read_holds_the_shard() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    let late = Late(&rig, PointInTimeSource::new(&rig.pits, 100));
    let (_, snap) = rig.sources();
    let done = rig.sweeper(LIVE, &late, &snap).slice(100).unwrap();
    assert_eq!((done.pinned, done.orphans), (1, 0));
    assert!(rig.exists(&name("gone", U3, 0)));
}

// AC-5c
#[test]
fn a_descriptor_created_after_the_page_was_listed_is_not_deleted() {
    // The slice reads the descriptor after listing: a container whose index
    // now lives under its uuid is never a candidate.
    let rig = Rig::new();
    rig.container(&name("late", U1, 0));
    rig.live("late", U1);
    let (pit, snap) = rig.sources();
    let done = rig.sweeper(LIVE, &pit, &snap).slice(100).unwrap();
    assert_eq!((done.candidates, done.orphans), (0, 0));
    assert!(rig.exists(&name("late", U1, 0)));
}

/// A source that moves the cursor register as a Java node would.
struct Rival<'a>(&'a Rig);

impl PinSource for Rival<'_> {
    fn collect(&self, _: &mut IndexPins) -> Result<(), JanitorError> {
        let mut buf = [0u8; 64];
        match self.0.emulator.read_register(CURSOR, &mut buf).unwrap() {
            Some(read) => {
                self.0
                    .emulator
                    .swap_register(CURSOR, read.generation, b"zzz")
                    .unwrap();
            }
            None => {
                self.0.emulator.create_register(CURSOR, b"zzz").unwrap();
            }
        }
        Ok(())
    }
}

// AC-6
#[test]
fn a_slice_that_loses_its_swap_stops_and_deletes_nothing() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    let rival = Rival(&rig);
    let (_, snap) = rig.sources();
    let done = rig.sweeper(LIVE, &rival, &snap).slice(100).unwrap();
    assert!(done.lost_swap);
    assert_eq!(done.orphans, 0);
    assert!(rig.exists(&name("gone", U3, 0)));
    assert_eq!(rig.cursor().as_deref(), Some("zzz"));
}

// AC-7
#[test]
fn slices_walk_every_container_in_order_and_the_last_ends_the_cycle() {
    let rig = Rig::new();
    let names: Vec<String> = (0..5).map(|i| name(&format!("i{i}"), U3, 0)).collect();
    for n in &names {
        rig.container(n);
        rig.live(&n.split('#').next().unwrap().to_owned(), U3);
    }
    let (pit, snap) = rig.sources();
    let sweeper = rig.sweeper(LIVE, &pit, &snap);
    let first = sweeper.slice(2).unwrap();
    assert_eq!((first.containers, first.cycle_ended), (2, false));
    assert_eq!(rig.cursor().as_deref(), Some(names[1].as_str()));
    let second = sweeper.slice(2).unwrap();
    assert_eq!(second.containers, 2);
    assert_eq!(rig.cursor().as_deref(), Some(names[3].as_str()));
    let third = sweeper.slice(2).unwrap();
    assert_eq!((third.containers, third.cycle_ended), (1, true));
    assert_eq!(rig.cursor().as_deref(), Some(""));
    // The next slice starts over.
    assert_eq!(sweeper.slice(2).unwrap().containers, 2);
}

// AC-8
#[test]
fn a_dry_run_counts_the_orphans_and_deletes_nothing() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    let (pit, snap) = rig.sources();
    let dry = Config {
        dry_run: true,
        ..LIVE
    };
    let done = rig.sweeper(dry, &pit, &snap).slice(100).unwrap();
    assert_eq!((done.orphans, done.blobs), (1, 3));
    assert!(rig.exists(&name("gone", U3, 0)));
    assert_eq!(rig.count(S3Op::Delete), 0);
}

// AC-9
#[test]
fn a_slice_costs_what_the_table_says_and_a_page_with_no_candidate_reads_no_pin() {
    let rig = Rig::new();
    rig.live("logs", U1);
    rig.container(&name("logs", U1, 0));
    rig.put("d/snapshots/never-read", b"{not read");
    let (pit, snap) = rig.sources();
    let sweeper = rig.sweeper(LIVE, &pit, &snap);
    let before = rig.emulator.meter().total();
    let (lists, puts, gets) = (
        rig.count(S3Op::List),
        rig.count(S3Op::Put),
        rig.count(S3Op::Get),
    );
    sweeper.slice(10).unwrap();
    // One container: one LIST of one key and one that found the end; the
    // cursor read and the descriptor read; the cursor created. No pin: the
    // unreadable snapshot was never touched, or the slice would have failed.
    assert_eq!(rig.count(S3Op::List) - lists, 2);
    assert_eq!(rig.count(S3Op::Put) - puts, 1);
    assert_eq!(rig.count(S3Op::Get) - gets, 2);
    budget::check(
        "janitor.orphan_slice.requests",
        rig.emulator.meter().total() - before,
        "a slice over one live container",
    );
}

// AC-9
#[test]
fn an_orphan_costs_its_listing_its_deletes_and_the_pins_reads() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    rig.snapshot("r#s", "other", U1, true);
    let (pit, snap) = rig.sources();
    let sweeper = rig.sweeper(LIVE, &pit, &snap);
    let before = rig.emulator.meter().total();
    let (lists, deletes, gets) = (
        rig.count(S3Op::List),
        rig.count(S3Op::Delete),
        rig.count(S3Op::Get),
    );
    sweeper.slice(10).unwrap();
    // Listing: 2. Pins: one page of points in time and one of snapshots.
    // The orphan: one LIST of its blobs.
    assert_eq!(rig.count(S3Op::List) - lists, 2 + 2 + 1);
    // One batch of two blobs, then the register.
    assert_eq!(rig.count(S3Op::Delete) - deletes, 2);
    // The cursor, the descriptor, the snapshot.
    assert_eq!(rig.count(S3Op::Get) - gets, 3);
    // And the cursor's creation.
    budget::check(
        "janitor.orphan_slice.orphan.requests",
        rig.emulator.meter().total() - before,
        "a slice over one orphan",
    );
}

// AC-11
#[test]
fn a_page_that_fails_three_times_is_skipped_counted_and_the_next_page_follows() {
    let rig = Rig::new();
    rig.container(&name("gone", U3, 0));
    rig.container(&name("later", U3, 0));
    rig.put("d/snapshots/bad", b"{");
    let (pit, snap) = rig.sources();
    let sweeper = rig.sweeper(LIVE, &pit, &snap);
    assert!(sweeper.slice(1).is_err());
    assert!(sweeper.slice(1).is_err());
    let skipped = sweeper.slice(1).unwrap();
    assert!(skipped.skipped.is_some());
    assert_eq!(sweeper.pages_skipped(), 1);
    assert!(
        rig.exists(&name("gone", U3, 0)),
        "a skipped page deletes nothing"
    );
    assert_eq!(rig.cursor().as_deref(), Some(name("gone", U3, 0).as_str()));
}
