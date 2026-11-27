//! docs/specs/virtual-time-soak/SPEC.md AC-6: a writer holding published
//! shards with nothing written makes no request for seven virtual days of its
//! periodic passes, and its requests an hour do not grow over the week.

// ARITH: test arithmetic on counts and virtual milliseconds.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore as _, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::PointInTimeStore;
use oss_node::writer::{JanitorPass, PassLimits};
use proactive_support::*;

const SHARDS: u32 = 4;
/// The publish and hibernate passes run every 250 ms in a node; here every
/// 15 virtual seconds, which is every pass that has anything to decide on.
const TICK: u64 = 15_000;
const JANITOR_EVERY: u64 = 60_000;
const HOUR: u64 = 3_600_000;
const WEEK_HOURS: u64 = 7 * 24;

fn requests(c: &Cluster) -> u64 {
    [
        S3Op::Get,
        S3Op::GetRange,
        S3Op::Put,
        S3Op::List,
        S3Op::Delete,
    ]
    .into_iter()
    .map(|op| c.store.meter().count(op))
    .sum()
}

/// The janitor pass a node assembles, on the run's clock.
fn pass_on(
    c: &Cluster,
    role: &Arc<oss_node::writer::WriterRole>,
    clock: &Arc<AtomicU64>,
) -> JanitorPass {
    let pit_clock = Arc::clone(clock);
    let pits = PointInTimeStore::new(
        c.base(),
        "",
        Arc::new(move || pit_clock.load(Ordering::SeqCst)),
    )
    .expect("pits");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, 1 << 20)
            .expect("fits"),
    );
    let pass_clock = Arc::clone(clock);
    JanitorPass::new(
        c.base(),
        "",
        Arc::clone(role),
        pits,
        &budget,
        PassLimits {
            buffer_bytes: 1 << 16,
            max_views: 100,
        },
        Box::new(move || i64::try_from(pass_clock.load(Ordering::SeqCst)).unwrap_or(i64::MAX)),
    )
}

#[test]
fn a_week_of_passes_over_idle_held_shards_asks_for_nothing_and_does_not_grow() {
    let wall = std::time::Instant::now();
    let c = cluster();
    create_n(&c, "logs", "u1", SHARDS);
    let one = node(&c, "node-1", 1, false, None);
    for ids in ids_by_shard(SHARDS, 1) {
        write_id(&one.role, &ids[0]);
    }
    for shard in 0..SHARDS {
        one.role.refresh(&info_n(SHARDS), shard).expect("published");
    }
    let clock = Arc::new(AtomicU64::new(1_000_000_000_000));
    let pass = pass_on(&c, &one.role, &clock);
    let mut config = gc_config(false);
    // What the node ships: a held, swept shard is listed again once an hour at most.
    config.relist_after_millis = i64::try_from(HOUR).expect("hour");
    // The sweep that follows the publishes is work; the week after is idle.
    pass.run(config);
    pass.run(config);
    let mut per_hour = Vec::new();
    for _ in 0..WEEK_HOURS {
        let before = requests(&c);
        let end = clock.load(Ordering::SeqCst) + HOUR;
        while clock.load(Ordering::SeqCst) < end {
            clock.fetch_add(TICK, Ordering::SeqCst);
            let now = clock.load(Ordering::SeqCst);
            one.role.publish_due();
            one.role.hibernate_due(now);
            if now % JANITOR_EVERY < TICK {
                pass.run(config);
            }
        }
        per_hour.push(requests(&c) - before);
    }
    assert_eq!(per_hour.len() as u64, WEEK_HOURS);
    assert_eq!(
        per_hour.iter().sum::<u64>(),
        0,
        "an idle held index asked the store for something: {per_hour:?}"
    );
    assert_eq!(one.role.held_shards().len(), SHARDS as usize);
    // The control: the same passes do ask the store for something once a
    // document is waiting, so the zero above is not a loop that does nothing.
    let waiting = ids_by_shard(SHARDS, 2);
    write_id(&one.role, &waiting[0][0]);
    let before = requests(&c);
    for _ in 0..(HOUR / TICK) {
        clock.fetch_add(TICK, Ordering::SeqCst);
        one.role.refresh(&info_n(SHARDS), 0).expect("published");
        one.role.publish_due();
        pass.run(config);
    }
    assert!(
        requests(&c) > before,
        "a written document made no request in an hour of the same passes"
    );
    eprintln!(
        "{WEEK_HOURS} virtual hours, 0 requests, {:?} wall",
        wall.elapsed()
    );
}
