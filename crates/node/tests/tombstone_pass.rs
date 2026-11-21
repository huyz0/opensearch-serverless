//! docs/specs/node-tombstone-sweep: one node of the fleet removes the
//! tombstones of deleted indexes, on a wait, and the rest make no request.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{IndexRecord, NodeLease};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{BUCKET_MILLIS, DEFAULT_QUARANTINE_MILLIS, DeleteMode, DescriptorStore};
use oss_node::writer::{TombstoneParts, TombstonePass, TombstoneReport};
use oss_testkit::budget;

/// 2025-11-15T00:00Z, the start of an hour.
const T0: i64 = 1_763_164_800_000;
const EVERY: i64 = BUCKET_MILLIS;

struct Rig {
    store: Arc<Emulator>,
    now: Arc<AtomicI64>,
    live: Vec<NodeLease>,
}

fn lease(id: &str) -> NodeLease {
    let mut lease = NodeLease::new(id, "e", "a", 0);
    lease.set_roles(["ingest".to_owned()]);
    lease
}

/// The node of `ids` that sweeps.
fn sweeper(ids: &[&str]) -> String {
    let live: Vec<NodeLease> = ids.iter().map(|id| lease(id)).collect();
    oss_cluster::placement::reapers_for("tombstone-sweep", live.iter(), 1)[0].to_owned()
}

fn rig(ids: &[&str]) -> Rig {
    Rig {
        store: Arc::new(Emulator::new(64 << 20, 1)),
        now: Arc::new(AtomicI64::new(T0 + DEFAULT_QUARANTINE_MILLIS + 2 * EVERY)),
        live: ids.iter().map(|id| lease(id)).collect(),
    }
}

impl Rig {
    fn pass(&self, node: &str, every: i64, pool: usize) -> TombstonePass {
        self.pass_probing(node, every, pool, DeleteMode::Conditional)
    }

    fn pass_probing(&self, node: &str, every: i64, pool: usize, mode: DeleteMode) -> TombstonePass {
        let store: Arc<dyn BlobStore> = self.store.clone();
        let clock = Arc::clone(&self.now);
        TombstonePass::new(
            TombstoneParts {
                store,
                base: String::new(),
                node_id: node.to_owned(),
                budget: MemoryBudget::new(
                    &BudgetConfig::new(256 << 20)
                        .with_pool(Pool::RequestBodies, pool)
                        .expect("pool"),
                ),
                buffer_bytes: 1 << 16,
                every_millis: every,
                probe: Box::new(move || mode),
            },
            Box::new(move || clock.load(Ordering::SeqCst)),
        )
    }

    /// Creates `name` and deletes it at `at`, leaving a tombstone and a marker.
    fn tombstone(&self, name: &str, at: i64) {
        let store: Arc<dyn BlobStore> = self.store.clone();
        let d = DescriptorStore::new(store, "", DeleteMode::Tombstone).expect("descriptors");
        let mut buf = vec![0u8; 4096];
        let g = d
            .create(&IndexRecord::new(name, "u", 1), at, &mut buf)
            .expect("created");
        assert!(d.delete_if_unchanged(name, g, at).expect("deleted"));
    }

    fn advance(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
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

    fn markers(&self) -> usize {
        let mut n = 0;
        self.store
            // LIST: a test counting the markers left.
            .list_prefix("tombstones/", None, 1000, &mut |_, _| n += 1)
            .expect("list");
        n
    }

    fn registers(&self) -> bool {
        let mut buf = [0u8; 4096];
        ["old", "young"].into_iter().any(|name| {
            self.store
                .read_register(&format!("indices/{name}"), &mut buf)
                .expect("read")
                .is_some()
        })
    }
}

fn removed(report: &TombstoneReport) -> u64 {
    report.removed
}

#[test]
fn a_node_that_is_not_the_sweeper_makes_no_request() {
    let r = rig(&["node-1", "node-2", "node-3"]);
    r.tombstone("old", T0 + 5);
    let other = ["node-1", "node-2", "node-3"]
        .into_iter()
        .find(|id| *id != sweeper(&["node-1", "node-2", "node-3"]))
        .expect("another node");
    let pass = r.pass(other, EVERY, 64 << 20);
    r.advance(10 * EVERY);
    let before = r.requests();
    let report = pass.run(&r.live);
    assert!(!report.designated, "{report:?}");
    assert!(!report.swept);
    budget::check(
        "janitor.tombstone_sweep.other_nodes.requests",
        r.requests() - before,
        "a node that is not the sweeper",
    );
}

#[test]
fn the_sweeper_waits_out_its_interval_then_removes_what_is_old_and_keeps_what_is_young() {
    let ids = ["node-1", "node-2", "node-3"];
    let r = rig(&ids);
    let me = sweeper(&ids);
    r.tombstone("old", T0 + 5);
    // Deleted two hours ago: inside the six-hour quarantine.
    let young_at = r.now.load(Ordering::SeqCst) - 2 * EVERY;
    r.tombstone("young", young_at);
    let pass = r.pass(&me, EVERY, 64 << 20);

    // AC-2: not due, no request.
    let before = r.requests();
    let report = pass.run(&r.live);
    assert!(
        report.designated && !report.due && !report.swept,
        "{report:?}"
    );
    assert_eq!(r.requests(), before);

    // AC-3: due: the probe is the caller's, so the sweep's own requests show.
    r.advance(EVERY);
    let report = pass.run(&r.live);
    assert!(report.swept, "{report:?}");
    assert_eq!(removed(&report), 1, "{report:?}");
    assert_eq!(report.kept, 0);
    assert_eq!(r.markers(), 1, "only the young one's marker is left");
    assert!(r.registers(), "the young tombstone stays");

    // AC-4: the next sweep waits an interval again.
    let before = r.requests();
    let again = pass.run(&r.live);
    assert!(!again.due && !again.swept, "{again:?}");
    assert_eq!(r.requests(), before);
}

#[test]
fn a_sweep_with_nothing_deleted_is_one_list() {
    let ids = ["node-1", "node-2"];
    let r = rig(&ids);
    let pass = r.pass(&sweeper(&ids), EVERY, 64 << 20);
    r.advance(EVERY);
    let (before, lists) = (r.requests(), r.store.meter().count(S3Op::List));
    let report = pass.run(&r.live);
    assert!(report.swept, "{report:?}");
    budget::check(
        "janitor.tombstone_sweep.idle.requests",
        r.requests() - before,
        "a sweep with nothing to remove",
    );
    assert_eq!(r.store.meter().count(S3Op::List) - lists, 1);
}

#[test]
fn a_store_that_does_not_honour_a_conditional_delete_is_never_swept() {
    let ids = ["node-1", "node-2"];
    let r = rig(&ids);
    r.tombstone("old", T0 + 5);
    let pass = r.pass_probing(&sweeper(&ids), EVERY, 64 << 20, DeleteMode::Tombstone);
    r.advance(EVERY);
    let before = r.requests();
    let report = pass.run(&r.live);
    assert!(report.due && !report.swept, "{report:?}");
    assert_eq!(r.requests(), before, "no request on such a store");
    assert_eq!(r.markers(), 1);
    assert!(r.registers());
}

#[test]
fn a_pass_with_no_memory_for_its_buffer_sweeps_nothing_and_tries_again() {
    let ids = ["node-1", "node-2"];
    let r = rig(&ids);
    r.tombstone("old", T0 + 5);
    let me = sweeper(&ids);
    let pass = r.pass(&me, EVERY, 1);
    r.advance(EVERY);
    let before = r.requests();
    let report = pass.run(&r.live);
    assert!(!report.swept && report.skipped.is_some(), "{report:?}");
    assert_eq!(r.requests(), before);
    // The wait was not used up: a pass with room, from the same state, sweeps at once.
    let roomy = r.pass(&me, EVERY, 64 << 20);
    r.advance(EVERY);
    assert_eq!(roomy.run(&r.live).removed, 1);
}

#[test]
fn an_interval_of_zero_turns_the_pass_off() {
    let ids = ["node-1", "node-2"];
    let r = rig(&ids);
    r.tombstone("old", T0 + 5);
    let pass = r.pass(&sweeper(&ids), 0, 64 << 20);
    r.advance(100 * EVERY);
    let before = r.requests();
    let report = pass.run(&r.live);
    assert!(!report.due && !report.swept, "{report:?}");
    assert_eq!(r.requests(), before);
}

#[test]
fn the_role_moves_to_the_next_node_when_the_sweeper_leaves() {
    let ids = ["node-1", "node-2", "node-3"];
    let first = sweeper(&ids);
    let rest: Vec<&str> = ids.iter().copied().filter(|id| *id != first).collect();
    let second = sweeper(&rest);
    let mut r = rig(&ids);
    r.tombstone("old", T0 + 5);
    let gone = r.pass(&first, EVERY, 64 << 20);
    let next = r.pass(&second, EVERY, 64 << 20);
    r.advance(EVERY);
    r.live.retain(|l| l.node_id != first);
    assert!(!gone.run(&r.live).designated);
    let report = next.run(&r.live);
    assert!(report.designated && report.swept, "{report:?}");
    assert_eq!(report.removed, 1);
}
