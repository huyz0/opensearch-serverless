//! docs/specs/write-pipeline: a failed window and the groups behind it, and
//! the memory a pipeline holds, with PUTs held and released by the test.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oss_blob::emulator::faults::Latency;
use oss_blob::emulator::hold::Release;
use oss_blob::{BlobStore, S3Op};
use oss_node::writer::Takeover;
use oss_writer::bulk::ItemResult;
use oss_writer::index::Outcome;
use support::*;

fn landed(r: &ItemResult) -> bool {
    matches!(r, ItemResult::Written { .. })
}

/// A node over an index `logs`, one write made so its epoch is open.
fn rig(depth: usize) -> (Cluster, Node) {
    let c = cluster();
    create(&c, "logs", "u1");
    let store: Arc<dyn BlobStore> = c.store.clone();
    let one = node(
        &c,
        "node-1",
        1,
        false,
        Some(Takeover::from_leases(store, "", "node-1", 1, 1 << 20)),
    );
    one.role.set_pipeline_depth(depth);
    assert_eq!(seq(&write(&one.role, "warm")), 0);
    (c, one)
}

fn puts(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::Put)
}

/// `id` written on its own thread.
fn spawn_write(one: &Node, id: &'static str) -> std::thread::JoinHandle<ItemResult> {
    let role = Arc::clone(&one.role);
    std::thread::spawn(move || write(&role, id))
}

const WAIT: Duration = Duration::from_secs(10);

// AC-17
#[test]
fn a_window_that_lands_behind_a_failed_one_is_refused_either_way_it_ends() {
    for how in [Release::Land, Release::LandThenFail] {
        let (c, one) = rig(8);
        c.store.set_latency(S3Op::Put, Latency::default());
        // A's window, then B's, which is sent only after A's has ended: one
        // blob in flight.
        let holds = c.store.hold_next(S3Op::Put, 2);
        let a = spawn_write(&one, "a");
        assert!(holds[0].wait_arrived(WAIT), "A's window was sent");
        let b = spawn_write(&one, "b");
        std::thread::sleep(Duration::from_millis(300));
        assert!(!holds[1].wait_arrived(Duration::ZERO), "B's waits its turn");
        holds[0].release(Release::Fail);
        assert!(!landed(&a.join().expect("ends")), "A is refused");
        assert!(holds[1].wait_arrived(WAIT), "B's window followed");
        holds[1].release(how);
        let b = b.join().expect("ends");
        assert!(
            !landed(&b),
            "B is refused after A's failure ({how:?}): {b:?}"
        );
        // The shard reopens and takes writes. When B's blob is known to
        // have landed, the numbers go on past B's; when its outcome is
        // unknown it counts as unpublished and they do not.
        let again = (0..3)
            .map(|_| write(&one.role, "after"))
            .find(landed)
            .expect("the shard takes writes again");
        if how == Release::Land {
            assert!(seq(&again) > 2, "numbered past B: {again:?}");
        }
        // A node that takes over finds that write, whatever its number.
        let calls = Arc::new(AtomicUsize::new(0));
        let take = takeover(&c, &one, &calls);
        let two = node(&c, "node-2", 2, true, Some(take));
        match write(&two.role, "after") {
            ItemResult::Written { written, .. } => {
                assert_eq!(written.outcome, Outcome::Updated, "kept ({how:?})");
            }
            other => panic!("{other:?}"),
        }
    }
}

/// A bulk of `items` small documents streamed in slices of `slice` bytes of
/// `budget`, answering how many of its items landed.
fn bulk_of(
    role: &Arc<oss_node::writer::WriterRole>,
    budget: &oss_writer::bulk::BulkBudget,
    slice: usize,
    tag: &str,
    items: usize,
) -> usize {
    use oss_writer::bulk::{Bulk, Defaults, IdGenerator, StreamConfig, stream};
    let defaults = Defaults {
        index: Some("logs".into()),
        ..Defaults::default()
    };
    let config = StreamConfig {
        slice_bytes: slice,
        max_wait: Duration::from_secs(20),
    };
    let mut target = oss_node::writer::RoleTarget::new(role);
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 4096];
    let mut run = Bulk::new(&mut target, &mut ids, 1, false);
    let mut body = String::new();
    for n in 0..items {
        body.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{tag}-{n}\"}}}}\n{{\"n\":{n},\"t\":\"padding padding\"}}\n"
        ));
    }
    stream(
        &mut body.as_bytes(),
        &mut buffer,
        &defaults,
        budget,
        config,
        &mut run,
    )
    .expect("streams");
    run.finish()
        .expect("finishes")
        .iter()
        .filter(|item| landed(&item.result))
        .count()
}

/// Samples the bytes `budget` has granted until `stop`, answering the most.
fn peak_of(budget: &oss_writer::bulk::BulkBudget, stop: &std::sync::atomic::AtomicBool) -> usize {
    let mut peak = 0;
    while !stop.load(Ordering::Relaxed) {
        peak = peak.max(budget.in_use().0);
        std::thread::sleep(Duration::from_millis(1));
    }
    peak
}

// AC-18
#[test]
fn at_the_largest_depth_the_bytes_in_flight_never_pass_the_budget() {
    const CAPACITY: usize = 256 << 10;
    const SLICE: usize = 64 << 10;
    let (_c, one) = rig(64);
    let budget = oss_writer::bulk::BulkBudget::new(CAPACITY, 64);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (landed_each, peak) = std::thread::scope(|s| {
        let sampler = s.spawn(|| peak_of(&budget, &stop));
        let writers: Vec<_> = (0..64)
            .map(|n| {
                let (role, budget) = (Arc::clone(&one.role), budget.clone());
                s.spawn(move || bulk_of(&role, &budget, SLICE, &format!("m{n}"), 1))
            })
            .collect();
        let landed: Vec<usize> = writers
            .into_iter()
            .map(|w| w.join().expect("ends"))
            .collect();
        stop.store(true, Ordering::Relaxed);
        (landed, sampler.join().expect("ends"))
    });
    assert!(
        landed_each.iter().all(|n| *n == 1),
        "every bulk landed: {landed_each:?}"
    );
    assert!(peak <= CAPACITY, "{peak} bytes granted of {CAPACITY}");
    assert!(peak >= SLICE, "the budget was in use: {peak}");
    assert_eq!(budget.in_use(), (0, 0), "all given back");
    assert_eq!(one.role.max_seq_no("u1", 0), 64, "64 groups, each numbered");
}

// AC-19
#[test]
fn a_body_larger_than_its_share_is_applied_in_slices_within_the_budget() {
    const CAPACITY: usize = 128 << 10;
    const SLICE: usize = 64 << 10;
    // About 250 KiB of items against a 64 KiB share: several slices, each
    // a group, none holding more than the share.
    const ITEMS: usize = 3_000;
    let (c, one) = rig(8);
    c.store.set_latency(S3Op::Put, Latency::default());
    let before = puts(&c);
    let budget = oss_writer::bulk::BulkBudget::new(CAPACITY, 8);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (applied, peak) = std::thread::scope(|s| {
        let sampler = s.spawn(|| peak_of(&budget, &stop));
        let applied = bulk_of(&one.role, &budget, SLICE, "u", ITEMS);
        stop.store(true, Ordering::Relaxed);
        (applied, sampler.join().expect("ends"))
    });
    assert_eq!(applied, ITEMS, "every item applied");
    // Each slice is a group of its own, and each waits for its window.
    assert!(puts(&c) - before >= 3, "{} PUTs", puts(&c) - before);
    assert!(
        peak <= SLICE,
        "a slice never held more than its share: {peak}"
    );
    assert_eq!(budget.in_use(), (0, 0), "all given back");
}
