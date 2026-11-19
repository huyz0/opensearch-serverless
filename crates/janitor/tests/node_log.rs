//! docs/specs/janitor-node-log/SPEC.md: collecting a dead node's write log,
//! over the real membership, heads and descriptors on the emulator and fakes
//! of the two seams that live above this crate.

// ARITH: times and counts here are test constants and small sums; an
// overflow panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LeaseConfig, LivenessOracle, Membership};
use oss_format::{IndexRecord, NodeLease, NodeLogEpoch, ShardKey};
use oss_janitor::JanitorError;
use oss_janitor::node_log::{Collected, Collector, Commits, Committed, FencedLog, NodeLogs};
use oss_meta::{DeleteMode, DescriptorStore};

const BASE: &str = "d/";
const TTL: i64 = 30_000;
const T0: i64 = 1_700_000_000_000;
const DEAD: &str = "n2";

struct Clock(AtomicI64);

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn pause(&self, _: Duration) {}
}

/// What the fake log holds, and what was asked of it, in order.
#[derive(Default)]
struct Log {
    named: Vec<ShardKey>,
    max: HashMap<(String, u32), i64>,
    ends: BTreeMap<u64, u64>,
    calls: Mutex<Vec<String>>,
    fail_delete: bool,
    /// Runs when the epochs are fenced: where a test moves the lease.
    on_fence: Mutex<Option<Box<dyn Fn() + Send>>>,
}

struct Fenced(
    Vec<ShardKey>,
    HashMap<(String, u32), i64>,
    BTreeMap<u64, u64>,
);

impl FencedLog for Fenced {
    fn named(&self) -> Result<Vec<ShardKey>, JanitorError> {
        Ok(self.0.clone())
    }
    fn max_seq_no_of(&self, shard: &ShardKey) -> Result<i64, JanitorError> {
        Ok(*self
            .1
            .get(&(shard.index_uuid.to_string(), shard.shard))
            .unwrap_or(&-1))
    }
    fn ends(&self) -> Result<BTreeMap<u64, u64>, JanitorError> {
        Ok(self.2.clone())
    }
}

impl NodeLogs for Log {
    fn fence(
        &self,
        node: &str,
        epochs: &[NodeLogEpoch],
    ) -> Result<Box<dyn FencedLog + '_>, JanitorError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("fence {node} {}", epochs.len()));
        if let Some(hook) = self.on_fence.lock().unwrap().as_ref() {
            hook();
        }
        Ok(Box::new(Fenced(
            self.named.clone(),
            self.max.clone(),
            self.ends.clone(),
        )))
    }
    fn delete_range(&self, node: &str, epoch: u64, from: u64, to: u64) -> Result<(), JanitorError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("delete {node} {epoch} {from}..{to}"));
        if self.fail_delete {
            return Err(JanitorError::Log("delete failed".into()));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Published(HashMap<(String, u32), Committed>);

impl Commits for Published {
    fn committed(&self, _: &str, uuid: &str, shard: u32) -> Result<Committed, JanitorError> {
        Ok(*self
            .0
            .get(&(uuid.to_owned(), shard))
            .unwrap_or(&Committed::Absent))
    }
}

struct Rig {
    store: Arc<Emulator>,
    clock: Arc<Clock>,
    membership: Arc<Membership>,
    heads: ShardHeadStore,
    descriptors: DescriptorStore,
    log: Log,
    commits: Published,
}

fn epochs() -> Vec<NodeLogEpoch> {
    vec![
        NodeLogEpoch {
            epoch: 7,
            trim: 2,
            landed: 9,
        },
        NodeLogEpoch {
            epoch: 8,
            trim: 0,
            landed: 3,
        },
    ]
}

impl Rig {
    /// A fleet where `n2` has left epochs 7 and 8 and then died.
    fn new() -> Self {
        let store = Arc::new(Emulator::new(64 << 20, 1));
        let clock = Arc::new(Clock(AtomicI64::new(T0)));
        let config = LeaseConfig {
            ttl_millis: TTL,
            max_members: 16,
            conditional_deletes: false,
        };
        let node = |c: &Arc<Clock>| {
            let s: Arc<dyn BlobStore> = store.clone();
            let c: Arc<dyn LeaseClock> = c.clone();
            Arc::new(
                Membership::new(s, BASE, c, config)
                    .unwrap()
                    .with_probe_interval(Duration::ZERO),
            )
        };
        let mut lease = NodeLease::new(DEAD, "e2", "a2", 0);
        lease.set_roles(["index".to_owned()]);
        lease.node_log = epochs();
        node(&clock).renew(&lease).unwrap();
        clock.0.fetch_add(TTL + 1, Ordering::SeqCst);
        let membership = node(&clock);
        let oracle = Arc::new(LivenessOracle::new(node(&clock)));
        let s: Arc<dyn BlobStore> = store.clone();
        let c: Arc<dyn LeaseClock> = clock.clone();
        let heads = ShardHeadStore::new(s.clone(), BASE, c, TTL, oracle).unwrap();
        let descriptors = DescriptorStore::new(s, BASE, DeleteMode::Tombstone).unwrap();
        Self {
            store,
            clock,
            membership,
            heads,
            descriptors,
            log: Log {
                ends: BTreeMap::from([(7, 12), (8, 5)]),
                ..Log::default()
            },
            commits: Published::default(),
        }
    }

    /// Index `name` at `uuid`, one shard whose log holds up to `held`.
    fn shard(&mut self, name: &str, uuid: &str, held: i64) {
        let mut buf = vec![0u8; 4096];
        self.descriptors
            .create(&IndexRecord::new(name, uuid, 1), T0, &mut buf)
            .unwrap();
        self.log.named.push(ShardKey::new(uuid, 0, Some(name)));
        self.log.max.insert((uuid.to_owned(), 0), held);
    }

    fn commit(&mut self, uuid: &str, committed: Committed) {
        self.commits.0.insert((uuid.to_owned(), 0), committed);
    }

    fn run(&self, dry_run: bool) -> Collected {
        self.try_run(dry_run).unwrap()
    }

    fn try_run(&self, dry_run: bool) -> Result<Collected, JanitorError> {
        let now = self.clock.0.load(Ordering::SeqCst);
        let clock = move || now;
        let collector = Collector::new(
            &self.membership,
            &self.heads,
            &self.descriptors,
            &self.log,
            &self.commits,
            &clock,
            dry_run,
        );
        collector.collect(DEAD, &mut vec![0u8; 1 << 16])
    }

    fn lease_epochs(&self) -> usize {
        self.membership.read(DEAD).unwrap().unwrap().node_log.len()
    }

    fn calls(&self) -> Vec<String> {
        self.log.calls.lock().unwrap().clone()
    }

    fn count(&self, op: S3Op) -> u64 {
        self.store.meter().count(op)
    }
}

#[test]
fn a_dead_node_whose_shards_are_published_past_is_collected_lease_first() {
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 10);
    rig.commit("u1", Committed::Commit(Some(10)));
    assert_eq!(rig.run(false), Collected::Collected);
    assert_eq!(rig.lease_epochs(), 0);
    assert_eq!(
        rig.calls(),
        ["fence n2 2", "delete n2 7 2..13", "delete n2 8 0..6"]
    );
}

#[test]
fn a_shard_the_commit_has_not_caught_up_with_keeps_the_log() {
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 10);
    rig.commit("u1", Committed::Commit(Some(9)));
    assert_eq!(rig.run(false), Collected::StillNeeded);
    assert_eq!(rig.lease_epochs(), 2);
    assert_eq!(rig.calls(), ["fence n2 2"]);
}

#[test]
fn a_log_holding_only_sequence_number_zero_is_not_published_past_a_checkpoint_of_minus_one() {
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 0);
    rig.commit("u1", Committed::Commit(Some(-1)));
    assert_eq!(rig.run(false), Collected::StillNeeded);
    rig.commit("u1", Committed::Commit(Some(0)));
    assert_eq!(rig.run(false), Collected::Collected);
}

#[test]
fn no_commit_no_checkpoint_an_owner_or_no_name_each_keep_the_log() {
    // no commit at all
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 10);
    assert_eq!(rig.run(false), Collected::StillNeeded);
    // a commit that records no checkpoint
    rig.commit("u1", Committed::Commit(None));
    assert_eq!(rig.run(false), Collected::StillNeeded);
    // a head still the dead node's own
    rig.commit("u1", Committed::Commit(Some(10)));
    rig.heads
        .acquire("logs", 0, DEAD, "e2", Some("u1"))
        .unwrap();
    assert_eq!(rig.run(false), Collected::StillNeeded);
    assert_eq!(rig.lease_epochs(), 2);
    // a shard key that names no index
    let mut rig = Rig::new();
    rig.log.named.push(ShardKey::new("u9", 0, None::<&str>));
    assert_eq!(rig.run(false), Collected::StillNeeded);
    assert!(!rig.calls().iter().any(|c| c.starts_with("delete")));
}

#[test]
fn a_deleted_or_recreated_index_and_a_shard_the_log_holds_nothing_of_need_nothing() {
    let mut rig = Rig::new();
    // deleted: named in the log, no descriptor
    rig.log
        .named
        .push(ShardKey::new("gone", 0, Some("deleted")));
    // recreated under another uuid
    let mut buf = vec![0u8; 4096];
    rig.descriptors
        .create(&IndexRecord::new("logs", "u2", 1), T0, &mut buf)
        .unwrap();
    rig.log.named.push(ShardKey::new("u1", 0, Some("logs")));
    rig.log.max.insert(("u1".into(), 0), 99);
    // named, holds nothing of it
    rig.shard("quiet", "u3", -1);
    assert_eq!(rig.run(false), Collected::Collected);
}

#[test]
fn nothing_is_judged_for_a_live_node_an_absent_lease_or_a_lease_without_epochs() {
    let rig = Rig::new();
    // alive: the clock back inside the lease
    rig.clock.0.store(T0 + 1, Ordering::SeqCst);
    let before = rig.store.meter().total();
    assert_eq!(rig.run(false), Collected::Nothing);
    assert_eq!(rig.store.meter().total() - before, 1);
    assert!(rig.calls().is_empty());
    // a node nobody has heard of
    let now = T0;
    let clock = move || now;
    let collector = Collector::new(
        &rig.membership,
        &rig.heads,
        &rig.descriptors,
        &rig.log,
        &rig.commits,
        &clock,
        false,
    );
    assert_eq!(
        collector.collect("nobody", &mut vec![0u8; 4096]).unwrap(),
        Collected::Nothing
    );
    // forget the epochs, then there is nothing left to collect
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 1);
    rig.commit("u1", Committed::Commit(Some(1)));
    assert_eq!(rig.run(false), Collected::Collected);
    let before = rig.store.meter().total();
    assert_eq!(rig.run(false), Collected::Nothing);
    assert_eq!(rig.store.meter().total() - before, 1);
}

#[test]
fn a_node_that_comes_back_before_the_forget_keeps_its_log() {
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 10);
    rig.commit("u1", Committed::Commit(Some(10)));
    // the fence is where the dead node renews its lease: live again
    let store: Arc<dyn BlobStore> = rig.store.clone();
    let clock: Arc<dyn LeaseClock> = rig.clock.clone();
    let config = LeaseConfig {
        ttl_millis: TTL,
        max_members: 16,
        conditional_deletes: false,
    };
    let back = Membership::new(store, BASE, clock, config).unwrap();
    let mut lease = NodeLease::new(DEAD, "e2", "a2", 0);
    lease.set_roles(["index".to_owned()]);
    lease.node_log = epochs();
    *rig.log.on_fence.lock().unwrap() = Some(Box::new(move || {
        back.renew(&lease).unwrap();
    }));
    assert_eq!(rig.run(false), Collected::StillNeeded);
    assert_eq!(rig.lease_epochs(), 2);
    assert_eq!(rig.calls(), ["fence n2 2"]);
}

#[test]
fn a_dry_run_judges_the_same_and_changes_nothing() {
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 10);
    rig.commit("u1", Committed::Commit(Some(10)));
    assert_eq!(rig.run(true), Collected::WouldCollect);
    assert_eq!(rig.lease_epochs(), 2);
    assert_eq!(rig.calls(), ["fence n2 2"]);
    assert_eq!(rig.count(S3Op::Delete), 0);
    // and a log still needed is still needed
    rig.commit("u1", Committed::Commit(Some(1)));
    assert_eq!(rig.run(true), Collected::StillNeeded);
    // switching the dry run off then collects
    rig.commit("u1", Committed::Commit(Some(10)));
    assert_eq!(rig.run(false), Collected::Collected);
}

#[test]
fn the_requests_are_the_lease_the_heads_and_descriptors_and_the_forget() {
    let mut rig = Rig::new();
    rig.shard("a", "u1", 1);
    rig.shard("b", "u2", 1);
    rig.commit("u1", Committed::Commit(Some(1)));
    rig.commit("u2", Committed::Commit(Some(1)));
    let before = rig.store.meter().total();
    let lists = rig.count(S3Op::List);
    assert_eq!(rig.run(false), Collected::Collected);
    // lease read; per shard a descriptor read and a head read; the forget's
    // lease read and swap
    assert_eq!(rig.store.meter().total() - before, 1 + 2 * 2 + 2);
    assert_eq!(rig.count(S3Op::List), lists);
}

#[test]
fn a_failed_delete_after_the_forget_is_an_error_and_the_lease_stays_forgotten() {
    let mut rig = Rig::new();
    rig.shard("logs", "u1", 10);
    rig.commit("u1", Committed::Commit(Some(10)));
    rig.log.fail_delete = true;
    assert!(rig.try_run(false).is_err());
    assert_eq!(rig.lease_epochs(), 0);
}
