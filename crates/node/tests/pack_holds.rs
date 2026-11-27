//! docs/specs/pack-collector: a live point in time or a snapshot holds its
//! index incarnation's packs, the answer is read once a pass and only when
//! asked, and a record that cannot be read is an error.

#![cfg(feature = "writer")]

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{PitShard, PointInTime};
use oss_meta::PointInTimeStore;
use oss_node::pack_holds::{HoldLimits, PassHolds};
use oss_writer::pack::IndexHolds;

const LIMITS: HoldLimits = HoldLimits {
    max_records: 100,
    max_pins: 100,
    record_bytes: 1 << 16,
};
const ID: &str = "abcdefghijklmnopqrstuv";

struct Rig {
    emulator: Arc<Emulator>,
    pits: Arc<PointInTimeStore>,
}

impl Rig {
    fn new() -> Self {
        let emulator = Arc::new(Emulator::new(64 << 20, 1));
        let store = Arc::clone(&emulator) as Arc<dyn BlobStore>;
        let pits = PointInTimeStore::new(store, "", Arc::new(|| 1_000)).expect("pits");
        Self {
            emulator,
            pits: Arc::new(pits),
        }
    }

    fn holds(&self) -> PassHolds {
        PassHolds::new(
            Arc::clone(&self.pits),
            Arc::clone(&self.emulator) as Arc<dyn BlobStore>,
            "",
            LIMITS,
        )
    }

    fn snapshot(&self, key: &str, body: &[u8]) {
        self.emulator
            .put(key, &mut SliceSource::new(body), &mut [0u8; 16])
            .expect("put");
    }

    fn pit(&self, index: &str, uuid: &str) {
        let pit = PointInTime {
            id: ID.to_owned(),
            index: index.to_owned(),
            index_uuid: Some(uuid.to_owned()),
            expires_at_millis: 1_000_000,
            capturing: false,
            shards: vec![PitShard {
                shard: 0,
                term: 1,
                files: Vec::new(),
            }],
        };
        self.pits
            .create(&pit, &mut vec![0u8; 1 << 16])
            .expect("create");
    }

    fn requests(&self) -> u64 {
        S3Op::ALL
            .iter()
            .map(|op| self.emulator.meter().count(*op))
            .sum()
    }
}

#[test]
fn a_live_point_in_time_holds_its_incarnation_and_no_other() {
    let rig = Rig::new();
    rig.pit("logs", "u1");
    let holds = rig.holds();
    assert!(holds.holds("logs", "u1").expect("answers"));
    assert!(!holds.holds("logs", "u2").expect("answers"));
    assert!(!holds.holds("metrics", "u1").expect("answers"));
}

#[test]
fn a_snapshot_holds_its_incarnation_and_no_other() {
    let rig = Rig::new();
    rig.snapshot(
        "snapshots/s1",
        br#"{"name":"s1","indices":{"logs":{"uuid":"u1"}}}"#,
    );
    let holds = rig.holds();
    assert!(holds.holds("logs", "u1").expect("answers"));
    assert!(!holds.holds("logs", "other").expect("answers"));
}

#[test]
fn nothing_is_read_until_asked_and_the_records_are_read_once() {
    let rig = Rig::new();
    rig.pit("logs", "u1");
    let before = rig.requests();
    let holds = rig.holds();
    assert_eq!(rig.requests(), before, "building a pass reads nothing");
    holds.holds("logs", "u1").expect("answers");
    let first = rig.requests();
    assert!(first > before);
    holds.holds("logs", "u1").expect("answers");
    holds.holds("metrics", "u9").expect("answers");
    assert_eq!(rig.requests(), first, "a second question reads nothing");
}

#[test]
fn a_snapshot_record_that_cannot_be_read_is_an_error() {
    let rig = Rig::new();
    rig.snapshot("snapshots/broken", b"not json");
    assert!(rig.holds().holds("logs", "u1").is_err());
}

#[test]
fn more_records_than_the_cap_is_an_error() {
    let rig = Rig::new();
    for n in 0..3 {
        rig.snapshot(
            &format!("snapshots/s{n}"),
            br#"{"indices":{"logs":{"uuid":"u1"}}}"#,
        );
    }
    let holds = PassHolds::new(
        Arc::clone(&rig.pits),
        Arc::clone(&rig.emulator) as Arc<dyn BlobStore>,
        "",
        HoldLimits {
            max_records: 2,
            ..LIMITS
        },
    );
    assert!(holds.holds("logs", "u1").is_err());
}
