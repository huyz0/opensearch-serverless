//! docs/specs/node-orphan-sweep: the node that the rendezvous names runs
//! the orphan-shard sweep on a timer, and no other node makes a request.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{IndexRecord, NodeLease};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::writer::{LiveLeases, OrphanParts, OrphanTimer};

const U1: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const U2: &str = "BBBBBBBBBBBBBBBBBBBBBB";

fn lease(id: &str) -> NodeLease {
    let mut lease = NodeLease::new(id, "e", "a", 0);
    lease.set_roles(["ingest".to_owned()]);
    lease
}

fn members(ids: &[&str]) -> Arc<[NodeLease]> {
    ids.iter().map(|id| lease(id)).collect()
}

/// The node of `ids` that sweeps.
fn sweeper(ids: &[&str]) -> String {
    let live = members(ids);
    oss_cluster::placement::reapers_for("orphan-sweep", live.iter(), 1)[0].to_owned()
}

struct Rig {
    store: Arc<Emulator>,
}

impl Rig {
    fn new() -> Self {
        let rig = Self {
            store: Arc::new(Emulator::new(64 << 20, 1)),
        };
        // a live index with a container, and an orphan with one
        let descriptors =
            DescriptorStore::new(rig.dyn_store(), "", DeleteMode::Tombstone).expect("descriptors");
        descriptors
            .create(&IndexRecord::new("live", U1, 1), 1, &mut vec![0u8; 1 << 16])
            .expect("created");
        rig.container(&format!("live#{U1}#0"));
        rig.container(&format!("gone#{U2}#0"));
        rig
    }

    fn dyn_store(&self) -> Arc<dyn BlobStore> {
        self.store.clone()
    }

    fn container(&self, name: &str) {
        let mut buf = [0u8; 64];
        self.store
            .put(
                &format!("segments/{name}/a"),
                &mut SliceSource::new(b"x"),
                &mut buf,
            )
            .expect("put");
    }

    fn exists(&self, name: &str) -> bool {
        let mut any = false;
        // LIST: the test's own look at what is left.
        self.store
            .list_prefix(&format!("segments/{name}/"), None, 10, &mut |_, _| {
                any = true;
            })
            .expect("list");
        any
    }

    fn requests(&self) -> u64 {
        [
            S3Op::Get,
            S3Op::GetRange,
            S3Op::Put,
            S3Op::List,
            S3Op::Delete,
        ]
        .into_iter()
        .map(|op| self.store.meter().count(op))
        .sum()
    }

    fn parts(&self, node: &str, slice: usize, dry_run: bool, pool: usize) -> OrphanParts {
        let now = Arc::new(AtomicI64::new(1_000));
        OrphanParts {
            store: self.dyn_store(),
            base: String::new(),
            node_id: node.to_owned(),
            budget: MemoryBudget::new(
                &BudgetConfig::new(256 << 20)
                    .with_pool(Pool::RequestBodies, pool)
                    .expect("pool"),
            ),
            slice,
            every: Duration::from_millis(10),
            dry_run,
            clock: Arc::new(move || u64::try_from(now.load(Ordering::SeqCst)).unwrap_or(0)),
            buckets: None,
            bucket_grace_millis: 0,
        }
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_budget_of_zero_starts_nothing() {
    let rig = Rig::new();
    let before = rig.requests();
    let live = Arc::new(LiveLeases::default());
    live.set(members(&["n1"]));
    assert!(OrphanTimer::spawn(rig.parts("n1", 0, false, 64 << 20), live).is_none());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.requests(), before);
    assert!(rig.exists(&format!("gone#{U2}#0")));
}

#[test]
fn the_named_node_deletes_an_orphan_and_leaves_a_live_index() {
    let rig = Rig::new();
    let ids = ["n1", "n2", "n3"];
    let named = sweeper(&ids);
    let live = Arc::new(LiveLeases::default());
    live.set(members(&ids));
    let _timer = OrphanTimer::spawn(rig.parts(&named, 100, false, 64 << 20), live).expect("on");
    until("the orphan is deleted", || {
        !rig.exists(&format!("gone#{U2}#0"))
    });
    assert!(rig.exists(&format!("live#{U1}#0")));
}

#[test]
fn a_node_the_rendezvous_does_not_name_makes_no_request() {
    let rig = Rig::new();
    let ids = ["n1", "n2", "n3"];
    let named = sweeper(&ids);
    let other = ids.iter().find(|id| **id != named).expect("another");
    let before = rig.requests();
    let live = Arc::new(LiveLeases::default());
    live.set(members(&ids));
    let _timer = OrphanTimer::spawn(rig.parts(other, 100, false, 64 << 20), live).expect("on");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.requests(), before);
    assert!(rig.exists(&format!("gone#{U2}#0")));
}

#[test]
fn a_dry_run_claims_the_cursor_and_deletes_nothing() {
    let rig = Rig::new();
    let live = Arc::new(LiveLeases::default());
    live.set(members(&["n1"]));
    let _timer = OrphanTimer::spawn(rig.parts("n1", 100, true, 64 << 20), live).expect("on");
    until("the cursor is claimed", || {
        let mut buf = [0u8; 256];
        rig.store
            .read_register("gc/orphan-sweep-cursor", &mut buf)
            .expect("read")
            .is_some()
    });
    assert!(rig.exists(&format!("gone#{U2}#0")));
    assert!(rig.exists(&format!("live#{U1}#0")));
}

#[test]
fn a_pool_without_room_makes_no_request() {
    let rig = Rig::new();
    let before = rig.requests();
    let live = Arc::new(LiveLeases::default());
    live.set(members(&["n1"]));
    let _timer = OrphanTimer::spawn(rig.parts("n1", 100, false, 4 << 10), live).expect("on");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.requests(), before);
    assert!(rig.exists(&format!("gone#{U2}#0")));
}
