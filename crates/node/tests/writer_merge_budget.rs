//! docs/specs/writer-segment-merge: what thirty refreshes of one shard cost in
//! requests with merging on and off.

// ARITH: test arithmetic on counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use oss_blob::{BlobStore, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::writer::MergeConfig;
use proactive_support::*;

const REFRESHES: u32 = 30;

/// The requests `REFRESHES` write-and-refresh rounds make, and the merges
/// that ran.
fn rounds(merge: bool) -> (u64, u64, u64) {
    let c = cluster();
    create_n(&c, "logs", "u1", 1);
    let one = node(&c, "node-1", 1, false, None);
    if merge {
        let mut config = MergeConfig::off();
        config.enabled = true;
        let budget = MemoryBudget::new(
            &BudgetConfig::new(256 << 20)
                .with_pool(Pool::OpenIndexes, 128 << 20)
                .expect("fits"),
        );
        one.role.set_merging(&config, &budget);
    }
    let info = info_n(1);
    one.role.hold(&info, 0).expect("holds");
    let (before, lists_before) = (c.store.meter().total(), c.store.meter().count(S3Op::List));
    for round in 0..REFRESHES {
        write_id(&one.role, &format!("doc-{round}"));
        one.role.refresh(&info, 0).expect("published");
    }
    let total = c.store.meter().total() - before;
    assert_eq!(c.store.meter().count(S3Op::List), lists_before, "no LIST");
    let puts = c.store.meter().count(S3Op::Put);
    (total, puts, one.role.merges_started())
}

// docs/specs/writer-segment-merge AC-9
#[test]
fn merging_costs_no_more_requests_than_not_merging() {
    let (off, off_puts, _) = rounds(false);
    let (on, on_puts, started) = rounds(true);
    eprintln!("off {off} requests ({off_puts} PUT), on {on} ({on_puts} PUT), {started} merges");
    assert!(started > 0, "merging ran");
    assert_eq!(on, off, "a merge makes no request");
    assert_eq!(on_puts, off_puts);
    oss_testkit::budget::check(
        "write.merge.requests",
        on,
        "thirty write-and-refresh rounds",
    );
    assert_eq!(on_puts, 101);
}

/// The sorted per-round latencies of `rounds` write-and-refresh rounds.
fn latencies(merge: bool, rounds: u32) -> Vec<std::time::Duration> {
    let c = cluster();
    create_n(&c, "logs", "u1", 1);
    let one = node(&c, "node-1", 1, false, None);
    if merge {
        let mut config = MergeConfig::off();
        config.enabled = true;
        let budget = MemoryBudget::new(
            &BudgetConfig::new(256 << 20)
                .with_pool(Pool::OpenIndexes, 128 << 20)
                .expect("fits"),
        );
        one.role.set_merging(&config, &budget);
    }
    let info = info_n(1);
    let mut taken = Vec::new();
    for round in 0..rounds {
        let start = std::time::Instant::now();
        write_id(&one.role, &format!("doc-{round}"));
        one.role.refresh(&info, 0).expect("published");
        taken.push(start.elapsed());
    }
    taken.sort();
    taken
}

// docs/specs/writer-segment-merge AC-8: run with --ignored --nocapture;
// the numbers are provisional (in-process emulator, shared machine).
#[test]
#[ignore = "a measurement, not a gate"]
fn write_and_refresh_latency_with_merging_on_and_off() {
    for _ in 0..3 {
        let off = latencies(false, 600);
        let on = latencies(true, 600);
        let at = |v: &[std::time::Duration], p: usize| v[v.len() * p / 100 - 1];
        eprintln!(
            "AC8 off p50 {:?} p99 {:?} max {:?} | on p50 {:?} p99 {:?} max {:?}",
            at(&off, 50),
            at(&off, 99),
            off.last().expect("some"),
            at(&on, 50),
            at(&on, 99),
            on.last().expect("some")
        );
    }
}
