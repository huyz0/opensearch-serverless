//! docs/specs/proactive-takeover-faults: a dead node's shards under the
//! faults a fleet meets: a take that fails, two reapers racing, a stale
//! index, a deadline, a cap on concurrency.

#[path = "proactive_support/mod.rs"]
mod support;

use oss_writer::takeover::Predecessors;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use oss_blob::S3Op;
use oss_blob::emulator::faults::{Faults, Latency};
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
