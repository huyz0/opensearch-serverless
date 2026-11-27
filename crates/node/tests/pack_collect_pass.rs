//! docs/specs/pack-collector: the node's collect pass runs the writer's
//! collector over the live membership, with references from commits and
//! from the points in time and snapshots, and deletes a pack nothing names.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_format::{
    FileLocation, ManifestEntry, PackSidecar, PitShard, PlannedBlock, PointInTime, ShardRef,
    SidecarEpoch, descriptor_key, encode_manifest, encode_node_pack, node_pack_key,
};
use oss_meta::PointInTimeStore;
use oss_node::pack_holds::HoldLimits;
use oss_node::pack_pass::{PackPass, PackPassParts};
use oss_writer::pack::{PackIndexStore, PackSidecarStore, WINDOW_MILLIS, collector_for, window_of};

const MIN: u64 = 60_000;
const WRITER: &str = "writer";
const LIMITS: HoldLimits = HoldLimits {
    max_records: 100,
    max_pins: 100,
    record_bytes: 1 << 16,
};

struct Rig {
    emulator: Arc<Emulator>,
    clock: Arc<AtomicU64>,
    members: Vec<String>,
}

impl Rig {
    fn new() -> Self {
        let emulator = Arc::new(Emulator::new(64 << 20, 1));
        // A pack in epoch 1, named in the index, whose header is not a pack's.
        let key = node_pack_key("", WRITER, 1, 0).expect("key");
        emulator
            .put(&key, &mut SliceSource::new(b"pack"), &mut [0; 16])
            .expect("put");
        let store = Arc::clone(&emulator) as Arc<dyn BlobStore>;
        PackIndexStore::new(Arc::clone(&store), "")
            .add(WRITER, 1)
            .expect("indexed");
        let sidecar = PackSidecarStore::new(store, "", WRITER).expect("sidecar");
        let state = sidecar.read().expect("read");
        sidecar
            .update(state, |_| PackSidecar::new(vec![SidecarEpoch::new(1, 1)]))
            .expect("update");
        Self {
            emulator,
            clock: Arc::new(AtomicU64::new(0)),
            members: (0..3).map(|n| format!("node-{n}")).collect(),
        }
    }

    fn pass(&self, me: &str) -> PackPass {
        let store = Arc::clone(&self.emulator) as Arc<dyn BlobStore>;
        let clock = Arc::clone(&self.clock);
        let pits = PointInTimeStore::new(Arc::clone(&store), "", Arc::new(|| 1_000)).expect("pits");
        let members = self.members.clone();
        PackPass::new(PackPassParts {
            store,
            base: String::new(),
            node_id: me.to_owned(),
            pits: Arc::new(pits),
            limits: LIMITS,
            max_tracked: 1_000,
            members: Box::new(move || members.clone()),
            now: Box::new(move || clock.load(Ordering::SeqCst)),
        })
    }

    fn elected(&self, window: u64) -> String {
        collector_for(window, &self.members)
            .expect("elected")
            .to_owned()
    }

    fn at(&self, window: u64) {
        self.clock.store(
            window.saturating_mul(WINDOW_MILLIS).saturating_add(MIN),
            Ordering::SeqCst,
        );
        assert_eq!(window_of(self.clock.load(Ordering::SeqCst)), window);
    }

    fn present(&self) -> bool {
        let key = node_pack_key("", WRITER, 1, 0).expect("key");
        self.emulator.head(&key).expect("head").is_some()
    }
}

/// An unreadable pack is an error, never "not named": the pass stops and
/// deletes nothing.
#[test]
fn a_pack_whose_header_cannot_be_read_stops_the_pass() {
    let rig = Rig::new();
    rig.emulator.advance_clock(16 * MIN);
    rig.at(100);
    let me = rig.elected(100);
    assert!(rig.pass(&me).run().is_err());
    assert!(rig.present());
}

#[test]
fn a_node_that_is_not_elected_does_nothing() {
    let rig = Rig::new();
    rig.at(100);
    let elected = rig.elected(100);
    let other = rig.members.iter().find(|m| **m != elected).expect("other");
    assert!(!rig.pass(other).run().expect("runs").ran);
}

#[test]
fn the_elected_node_runs_the_pass_once_a_window() {
    let rig = Rig::new();
    rig.at(100);
    let pass = rig.pass(&rig.elected(100));
    assert!(pass.run().expect("runs").ran);
    assert!(!pass.run().expect("runs").ran, "the window has run");
}

/// A real pack for shard 0 of `logs` (uuid `u1`), with the descriptor of
/// that index; the shard's commit names the pack when `named`.
fn real_pack(rig: &Rig, named: bool) {
    let planned = [PlannedBlock {
        index_uuid: "u1",
        index_name: "logs",
        shard: 0,
        name: "_0.seg",
        length: 10,
    }];
    let mut bytes = encode_node_pack(WRITER, 1, 0, &planned)
        .expect("encode")
        .bytes;
    bytes.extend_from_slice(&[0; 10]);
    let put = |key: String, body: &[u8]| {
        rig.emulator
            .put(&key, &mut SliceSource::new(body), &mut [0; 16])
            .expect("put");
    };
    put(node_pack_key("", WRITER, 1, 0).expect("key"), &bytes);
    let register = |value: &[u8]| {
        let mut blob = 0u64.to_be_bytes().to_vec();
        blob.extend_from_slice(value);
        blob
    };
    put(
        descriptor_key("", "logs").expect("key"),
        &register(br#"{"name":"logs","uuid":"u1","number_of_shards":1}"#),
    );
    let ordinal = u64::from(!named);
    let location = FileLocation::NodePacked {
        node: WRITER,
        epoch: 1,
        ordinal,
        offset: 0,
        length: 10,
    };
    let commit = encode_manifest(
        1,
        Some(WRITER),
        &[ManifestEntry {
            name: "_0.cfs",
            location: location.encode(),
            length: 10,
        }],
        -1,
        -1,
    )
    .expect("manifest");
    let shard = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    put(shard.manifest_key("").expect("key"), &register(&commit));
}

/// Two windows the same node is elected for, so its sighting is remembered.
fn two_windows(rig: &Rig) -> (u64, u64, String) {
    let first = 100;
    let me = rig.elected(first);
    let second = (first + 1..200)
        .find(|w| collector_for(*w, &rig.members) == Some(me.as_str()))
        .expect("elected again");
    (first, second, me)
}

#[test]
fn a_pack_nothing_names_is_deleted_at_the_second_window() {
    let rig = Rig::new();
    real_pack(&rig, false);
    rig.emulator.advance_clock(16 * MIN);
    let (first, second, me) = two_windows(&rig);
    let pass = rig.pass(&me);
    rig.at(first);
    assert_eq!(pass.run().expect("one").deleted, 0);
    assert!(rig.present(), "seen once");
    rig.at(second);
    assert_eq!(pass.run().expect("two").deleted, 1);
    assert!(!rig.present());
}

#[test]
fn a_pack_a_commit_names_is_kept() {
    let rig = Rig::new();
    real_pack(&rig, true);
    rig.emulator.advance_clock(16 * MIN);
    let (first, second, me) = two_windows(&rig);
    let pass = rig.pass(&me);
    for window in [first, second] {
        rig.at(window);
        let out = pass.run().expect("runs");
        assert_eq!(out.deleted, 0);
    }
    assert!(rig.present());
}

#[test]
fn a_pack_a_live_point_in_time_holds_is_kept() {
    let rig = Rig::new();
    real_pack(&rig, false);
    let store = Arc::clone(&rig.emulator) as Arc<dyn BlobStore>;
    let pits = PointInTimeStore::new(store, "", Arc::new(|| 1_000)).expect("pits");
    let pit = PointInTime {
        id: "abcdefghijklmnopqrstuv".to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("u1".to_owned()),
        expires_at_millis: 1_000_000,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 1,
            files: Vec::new(),
        }],
    };
    pits.create(&pit, &mut vec![0u8; 1 << 16]).expect("create");
    rig.emulator.advance_clock(16 * MIN);
    let (first, second, me) = two_windows(&rig);
    let pass = rig.pass(&me);
    for window in [first, second] {
        rig.at(window);
        assert_eq!(pass.run().expect("runs").deleted, 0);
    }
    assert!(rig.present(), "held by the view");
}
