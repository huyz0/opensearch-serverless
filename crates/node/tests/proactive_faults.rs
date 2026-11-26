//! docs/specs/proactive-takeover-faults: a dead node's shards under the
//! faults a fleet meets: a take that fails, two reapers racing, a stale
//! index, a deadline, a cap on concurrency.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use oss_writer::takeover::Predecessors;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use oss_blob::emulator::faults::{Faults, Latency};
use oss_blob::{BlobStore as _, S3Op};
use oss_node::writer::{ProactiveConfig, ProactiveTakeover};
use support::*;

/// node-1 writes to every one of `shards` shards and dies, none published.
fn dead_writer(c: &Cluster, shards: u32) -> Node {
    create_n(c, "logs", "u1", shards);
    let one = node(c, "node-1", 1, false, None);
    for ids in ids_by_shard(shards, 1) {
        write_id(&one.role, &ids[0]);
    }
    one
}

/// A pass by `id` over `one`'s log with its own reader of it.
fn reaper(
    c: &Cluster,
    one: &Node,
    id: &str,
    term: u64,
    cfg: ProactiveConfig,
) -> (Node, ProactiveTakeover, Arc<Predecessors>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover_as(c, one, id, i64::try_from(term).expect("term"), &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let predecessors2 = Arc::clone(&predecessors);
    let me = node(c, id, term, true, Some(take));
    let pass = ProactiveTakeover::new(id, cfg, Arc::clone(&me.role), predecessors);
    (me, pass, predecessors2)
}

#[test]
fn every_shard_the_log_names_is_taken_in_one_pass() {
    let c = cluster();
    let one = dead_writer(&c, 3);
    let (two, pass, _) = reaper(&c, &one, "node-2", 2, config(true));
    let report = pass.pass(&dead(&one));
    assert_eq!(
        (report.named, report.taken, report.deferred),
        (3, 3, 0),
        "{report:?}"
    );
    assert_eq!(two.role.held_shards().len(), 3);
}

#[test]
fn a_shard_published_before_the_last_blob_is_not_named_and_not_touched() {
    let c = cluster();
    create_n(&c, "logs", "u1", 2);
    let ids = ids_by_shard(2, 2);
    let one = node(&c, "node-1", 1, false, None);
    write_id(&one.role, &ids[0][0]);
    one.role.refresh(&info_n(2), 0).expect("published");
    write_id(&one.role, &ids[1][0]);
    let (first, second) = (0, 1);
    let (two, pass, _) = reaper(&c, &one, "node-2", 2, config(true));
    let report = pass.pass(&dead(&one));
    assert_eq!((report.named, report.taken), (1, 1), "{report:?}");
    assert!(two.role.held("u1", second));
    assert!(!two.role.held("u1", first));
}

#[test]
fn a_take_that_fails_leaves_the_shard_to_the_next_write_with_nothing_lost() {
    let c = cluster();
    let one = first_writer(&c);
    let (two, pass, log) = reaper(&c, &one, "node-2", 2, config(true));
    // The fence is placed; what fails is the take.
    log.of("node-1")
        .expect("reads")
        .expect("log")
        .named()
        .expect("fenced");
    c.store.set_faults(
        S3Op::Put,
        Faults {
            fail_ppm: 1_000_000,
            throttle_ppm: 0,
        },
    );
    let lease = dead(&one);
    let report = pass.pass(&lease);
    assert_eq!((report.taken, report.deferred), (0, 1), "{report:?}");
    assert!(two.role.held_shards().is_empty());
    c.store.set_faults(S3Op::Put, Faults::default());
    // Not recorded as finished: a later refresh runs it again.
    let r = refreshed(&["node-2"], lease);
    assert_eq!(pass.due(&r, i64::MAX).len(), 1);
    // The next write takes it lazily and replays b: its seq follows a and b.
    assert_eq!(seq(&write(&two.role, "c")), 2);
}

#[test]
fn a_take_whose_answer_was_lost_is_finished_by_the_next_pass() {
    let c = cluster();
    let one = first_writer(&c);
    let (two, pass, log) = reaper(&c, &one, "node-2", 2, config(true));
    log.of("node-1")
        .expect("reads")
        .expect("log")
        .named()
        .expect("fenced");
    let lease = dead(&one);
    c.store.lose_response_next(S3Op::Put, 1);
    let first = pass.pass(&lease);
    assert_eq!(first.taken + first.deferred, 1, "{first:?}");
    let second = pass.pass(&lease);
    assert_eq!(second.deferred, 0, "{second:?}");
    assert_eq!(two.role.held_shards().len(), 1);
    assert_eq!(seq(&write(&two.role, "c")), 2);
}

#[test]
fn two_reapers_racing_leave_each_shard_held_once() {
    for round in 0..8 {
        let c = cluster();
        let one = dead_writer(&c, 4);
        let (two, p2, _) = reaper(&c, &one, "node-2", 2, config(true));
        let (three, p3, _) = reaper(&c, &one, "node-3", 3, config(true));
        let lease = dead(&one);
        let gate = Barrier::new(2);
        let (r2, r3) = std::thread::scope(|s| {
            let a = s.spawn(|| {
                gate.wait();
                p2.pass(&lease)
            });
            let b = s.spawn(|| {
                gate.wait();
                p3.pass(&lease)
            });
            (a.join().expect("2"), b.join().expect("3"))
        });
        let held = two.role.held_shards().len() + three.role.held_shards().len();
        assert_eq!(held, 4, "round {round}: {r2:?} {r3:?}");
        assert_eq!(r2.taken + r3.taken, 4, "round {round}: {r2:?} {r3:?}");
        for shard in two.role.held_shards() {
            assert!(!three.role.held(&shard.uuid, shard.shard), "held twice");
        }
    }
}

/// Every request the store has counted so far.
fn requests(c: &Cluster) -> u64 {
    S3Op::ALL.iter().map(|op| c.store.meter().count(*op)).sum()
}

// docs/specs/proactive-takeover AC-5: the second reaper takes only what is still needed, for at
// most twice the first's requests
#[test]
fn a_second_reaper_after_the_first_costs_at_most_twice_as_much_and_takes_nothing() {
    let c = cluster();
    let one = dead_writer(&c, 4);
    let (two, p2, _) = reaper(&c, &one, "node-2", 2, config(true));
    let (three, p3, _) = reaper(&c, &one, "node-3", 3, config(true));
    let lease = dead(&one);
    let before = requests(&c);
    let first = p2.pass(&lease);
    let first_cost = requests(&c) - before;
    assert_eq!((first.named, first.taken), (4, 4), "{first:?}");
    let before = requests(&c);
    let second = p3.pass(&lease);
    let second_cost = requests(&c) - before;
    assert_eq!(
        (second.taken, second.skipped),
        (0, 4),
        "nothing is left to take: {second:?}"
    );
    assert!(
        second_cost <= 2 * first_cost,
        "the second reaper cost {second_cost} requests, the first {first_cost}"
    );
    assert_eq!(two.role.held_shards().len(), 4);
    assert!(three.role.held_shards().is_empty());
}

// docs/specs/proactive-takeover AC-5: the first reaper is down, so the second takes them all
#[test]
fn a_second_reaper_takes_every_shard_when_the_first_never_runs() {
    let c = cluster();
    let one = dead_writer(&c, 4);
    let (_down, _, _) = reaper(&c, &one, "node-2", 2, config(true));
    let (three, p3, _) = reaper(&c, &one, "node-3", 3, config(true));
    let lease = dead(&one);
    let report = p3.pass(&lease);
    assert_eq!(
        (report.named, report.taken, report.deferred),
        (4, 4, 0),
        "{report:?}"
    );
    assert_eq!(three.role.held_shards().len(), 4);
}

#[test]
fn a_shard_a_live_node_holds_is_left_alone_and_not_run_again() {
    let c = cluster();
    let one = first_writer(&c);
    let (three, _, _) = reaper(&c, &one, "node-3", 3, config(false));
    assert_eq!(seq(&write(&three.role, "x")), 2, "node-3 took it lazily");
    let (two, pass, _) = reaper(&c, &one, "node-2", 2, config(true));
    let lease = dead(&one);
    let report = pass.pass(&lease);
    assert_eq!(
        (report.taken, report.deferred, report.skipped),
        (0, 0, 1),
        "{report:?}"
    );
    assert!(two.role.held_shards().is_empty());
    assert!(
        pass.due(&refreshed(&["node-2", "node-3"], lease), i64::MAX)
            .is_empty()
    );
}

#[test]
fn an_index_recreated_under_another_uuid_is_skipped() {
    let c = cluster();
    let one = first_writer(&c);
    let (two, pass, _) = reaper(&c, &one, "node-2", 2, config(true));
    c.descriptors.delete("logs").expect("deleted");
    create(&c, "logs", "u2");
    let report = pass.pass(&dead(&one));
    assert_eq!(
        (report.named, report.skipped, report.taken),
        (1, 1, 0),
        "{report:?}"
    );
    assert!(two.role.held_shards().is_empty());
}

#[test]
fn a_deadline_stops_new_takes_and_the_rest_stay_lazy() {
    let c = cluster();
    let one = dead_writer(&c, 3);
    let cfg = ProactiveConfig {
        max_concurrent: 1,
        pass: Duration::from_millis(150),
        ..config(true)
    };
    let (two, pass, log) = reaper(&c, &one, "node-2", 2, cfg);
    // The fence and the header are read before the clock starts; each take
    // is then about a quarter second, past the 150 ms deadline, and the
    // pass starts only the first.
    log.of("node-1")
        .expect("reads")
        .expect("log")
        .named()
        .expect("fenced");
    for op in [S3Op::Get, S3Op::Head, S3Op::GetRange, S3Op::Put] {
        c.store
            .set_latency(op, Latency::fixed(Duration::from_millis(20)));
    }
    let lease = dead(&one);
    let report = pass.pass(&lease);
    assert_eq!((report.taken, report.deferred), (1, 2), "{report:?}");
    assert_eq!(two.role.held_shards().len(), 1);
    // A deferred pass is run again.
    assert_eq!(pass.due(&refreshed(&["node-2"], lease), i64::MAX).len(), 1);
}

#[test]
fn a_pass_takes_no_more_shards_at_once_than_max_concurrent() {
    let latency = Duration::from_millis(20);
    // Every take is at least three requests in a row, so n takes one after
    // another cost at least 3 * n * latency.
    let floor = |takes: u32| latency * 3 * takes;
    for (cap, shards, rounds) in [(1usize, 4u32, 4u32), (2, 4, 2)] {
        let c = cluster();
        let one = dead_writer(&c, shards);
        let cfg = ProactiveConfig {
            max_concurrent: cap,
            ..config(true)
        };
        let (_two, pass, _) = reaper(&c, &one, "node-2", 2, cfg);
        let lease = dead(&one);
        for op in [S3Op::Get, S3Op::Head, S3Op::GetRange, S3Op::Put] {
            c.store.set_latency(op, Latency::fixed(latency));
        }
        let start = Instant::now();
        let report = pass.pass(&lease);
        assert_eq!(report.taken, shards as usize, "{report:?}");
        assert!(
            start.elapsed() >= floor(rounds),
            "cap {cap}: {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn a_header_over_the_cap_is_refused_and_its_shards_stay_lazy() {
    let c = cluster();
    let one = dead_writer(&c, 3);
    let (two, pass, _) = reaper(&c, &one, "node-2", 2, config(true));
    let pass = pass.with_max_header_bytes(64);
    let lease = dead(&one);
    let report = pass.pass(&lease);
    assert!(report.header_refused, "{report:?}");
    assert_eq!((report.named, report.taken), (0, 0), "{report:?}");
    assert!(two.role.held_shards().is_empty());
    // Finished: no refresh reads that header again.
    assert!(
        pass.due(&refreshed(&["node-2"], lease), i64::MAX)
            .is_empty()
    );
    // The first write to each shard still takes it, reading the header unbounded.
    for ids in ids_by_shard(3, 1) {
        assert!(write_id(&two.role, &ids[0]) >= 1);
    }
    assert_eq!(two.role.held_shards().len(), 3);
}

/// After node-2's pass took the dead node's shard, node-2's dead-log pass
/// collects the log: through the pass's readers or through its own. Returns
/// the requests and the report of that collection.
fn collect_after_a_pass(shared: bool) -> (u64, oss_node::writer::DeadLogReport) {
    use oss_blob::BlobStore;
    use oss_cluster::heads::ShardHeadStore;
    use oss_cluster::{LeaseConfig, LivenessOracle, Membership, SystemClock};
    use oss_mem::{BudgetConfig, MemoryBudget, Pool};
    use oss_node::writer::{DeadLogParts, DeadLogPass, DeadSnapshot};

    let c = cluster();
    let one = first_writer(&c);
    let (two, pass, predecessors) = reaper(&c, &one, "node-2", 2, config(true));
    let lease = dead(&one);
    assert_eq!(pass.pass(&lease).taken, 1);
    // Everything the log held is published past.
    two.role.refresh(&info(), 0).expect("published");
    let dynstore: Arc<dyn BlobStore> = c.store.clone();
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&dynstore),
            "",
            Arc::new(Later),
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 16,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    );
    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&dynstore),
            "",
            Arc::new(Later),
            30_000,
            Arc::new(LivenessOracle::new(Arc::clone(&membership))),
        )
        .expect("heads"),
    );
    let _ = SystemClock;
    let collecting = DeadLogPass::new(
        DeadLogParts {
            membership,
            heads,
            descriptors: Arc::clone(&c.descriptors),
            store: dynstore,
            base: String::new(),
            node_id: "node-2".to_owned(),
            term: 2,
            budget: MemoryBudget::new(
                &BudgetConfig::new(256 << 20)
                    .with_pool(Pool::RequestBodies, 64 << 20)
                    .expect("pool"),
            ),
            buffer_bytes: 1 << 16,
            first_wait_millis: 1_000,
            predecessors: shared.then_some(predecessors),
        },
        Box::new(|| i64::MAX / 2),
    );
    let snapshot = DeadSnapshot {
        live: refreshed(&["node-2"], lease.clone()).live,
        dead: vec![lease],
    };
    let before = c.store.meter().total();
    let report = collecting.run(&snapshot, false);
    (c.store.meter().total() - before, report)
}

#[test]
fn the_dead_log_pass_reuses_the_fence_the_proactive_pass_placed() {
    let (own, own_report) = collect_after_a_pass(false);
    let (shared, shared_report) = collect_after_a_pass(true);
    assert_eq!(own_report.collected, 1, "{own_report:?}");
    assert_eq!(shared_report.collected, 1, "{shared_report:?}");
    eprintln!("requests to collect: own fence {own}, shared {shared}");
    assert!(shared < own, "sharing saved no fence: {shared} vs {own}");
}

#[test]
fn a_node_that_died_returned_and_died_again_is_read_by_its_new_epochs() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let epochs = dead(&one).node_log;
    let made = || calls.load(std::sync::atomic::Ordering::SeqCst);
    take.predecessors
        .of_naming("node-1", &epochs)
        .expect("made");
    take.predecessors
        .of_naming("node-1", &epochs)
        .expect("kept");
    assert_eq!(made(), 1, "the same epochs are the same log");
    // A later trim point of the same epoch is still that log.
    let mut trimmed = epochs.clone();
    trimmed[0].trim = trimmed[0].trim.saturating_add(1);
    take.predecessors
        .of_naming("node-1", &trimmed)
        .expect("kept");
    assert_eq!(made(), 1);
    // The node came back under the same id with a new epoch.
    let mut again = epochs.clone();
    again[0].epoch = again[0].epoch.saturating_add(10);
    take.predecessors
        .of_naming("node-1", &again)
        .expect("remade");
    assert_eq!(made(), 2);
    // A collected log is forgotten.
    take.predecessors.forget("node-1");
    take.predecessors
        .of_naming("node-1", &again)
        .expect("remade");
    assert_eq!(made(), 3);
}

/// Routes that place every shard on this node, so a shard another node holds
/// is the opener's to find, and that record the forwards it makes.
struct Forwarding {
    inner: oss_node::descriptor_routes::DescriptorRoutes,
    sent: std::sync::Mutex<Vec<String>>,
}

impl oss_node::writer::Routes for Forwarding {
    fn index(&self, name: &str) -> Option<oss_writer::bulk::IndexInfo> {
        self.inner.index(name)
    }

    fn owner(&self, index: &oss_writer::bulk::IndexInfo, shard: u32) -> oss_writer::bulk::Owner {
        self.inner.owner(index, shard)
    }

    fn forward(
        &self,
        node: &str,
        _: &oss_writer::bulk::IndexInfo,
        _: u32,
        group: &[oss_writer::bulk::Planned<'_>],
    ) -> Vec<oss_writer::bulk::ItemResult> {
        self.sent.lock().expect("lock").push(node.to_owned());
        group
            .iter()
            .map(|_| oss_writer::bulk::ItemResult::Written {
                written: oss_writer::index::Written {
                    outcome: oss_writer::index::Outcome::Created,
                    version: 1,
                    seq_no: 0,
                    primary_term: 1,
                },
                forced_refresh: false,
            })
            .collect()
    }
}

#[test]
fn a_write_placed_on_another_node_reaches_the_reaper_that_took_the_shard() {
    let c = cluster();
    let one = first_writer(&c);
    let (two, pass, _) = reaper(&c, &one, "node-2", 2, config(true));
    assert_eq!(pass.pass(&dead(&one)).taken, 1);
    let routes = Arc::new(Forwarding {
        inner: oss_node::descriptor_routes::DescriptorRoutes::new(
            oss_meta::DescriptorStore::new(c.store.clone(), "", oss_meta::DeleteMode::Tombstone)
                .expect("names"),
            1 << 16,
        ),
        sent: std::sync::Mutex::new(Vec::new()),
    });
    let kept = Arc::clone(&routes);
    let three = node_routed(&c, "node-3", 3, true, None, move |_| kept);
    // The first write finds the head owned by node-2 (live), and goes there.
    let first = write(&three.role, "x");
    assert!(
        matches!(first, oss_writer::bulk::ItemResult::Written { .. }),
        "{first:?}"
    );
    assert_eq!(*routes.sent.lock().expect("lock"), ["node-2"]);
    assert!(!three.role.held("u1", 0), "not taken from the reaper");
    // The next one costs only the index lookup: the holder is remembered.
    let before = c.store.meter().total();
    write(&three.role, "y");
    assert_eq!(*routes.sent.lock().expect("lock"), ["node-2", "node-2"]);
    assert_eq!(c.store.meter().total() - before, 1, "the index lookup only");
    assert!(two.role.held("u1", 0));
}

#[test]
fn a_live_sweep_after_a_failed_take_and_after_a_lost_answer_deletes_nothing_the_commit_names() {
    for lose_answer in [false, true] {
        let c = cluster();
        let one = first_writer(&c);
        let (two, pass, log) = reaper(&c, &one, "node-2", 2, config(true));
        log.of("node-1")
            .expect("reads")
            .expect("log")
            .named()
            .expect("fenced");
        let lease = dead(&one);
        if lose_answer {
            c.store.lose_response_next(S3Op::Put, 1);
        } else {
            c.store.set_faults(
                S3Op::Put,
                Faults {
                    fail_ppm: 1_000_000,
                    throttle_ppm: 0,
                },
            );
        }
        pass.pass(&lease);
        c.store.set_faults(S3Op::Put, Faults::default());
        pass.pass(&lease);
        // The shard is taken now, or by the next write; either way published.
        assert_eq!(seq(&write(&two.role, "c")), 2);
        two.role.refresh(&info(), 0).expect("published");
        let orphan = plant(&c);
        let sweeper = janitor(&c, &two.role);
        sweeps_delete_only_what_the_commit_does_not_name(&c, &sweeper, &orphan);
    }
}
