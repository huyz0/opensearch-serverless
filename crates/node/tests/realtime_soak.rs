//! docs/specs/virtual-time-soak AC-7: the soak's driver on the real clock,
//! in seconds. It holds what a stepped clock cannot: real threads renewing
//! leases beside the writes, real timers and real pauses.
//!
//! The driver is the shape of `virtual_soak.rs` (a seeded owner, a kill or a
//! freeze, a take-over, a collection) generalised over a real clock; it is
//! copied rather than shared so that file stays free for its own slice.

// ARITH: test arithmetic on counts and real milliseconds.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use oss_blob::BlobStore as _;
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::writer::{
    DeadLogParts, DeadLogPass, DeadSnapshot, DescriptorIncarnation, MergeConfig, Takeover,
};
use oss_writer::bulk::ItemResult;
use support::*;

/// The lease TTL of the smoke run: short, so a cycle is seconds.
const TTL: i64 = 2_000;
/// A live node's lease timer, and the take-over poll.
const STEP: i64 = 250;
/// What a loaded CI runner may add to a take-over beyond the lease and two
/// polls: the bound is a lease, not a scheduler.
const SLACK: i64 = 1_500;
const FIRST_WAIT: i64 = 2_000;

/// A small seeded generator: the run is the same for one seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// One writer node of the run, with the handle that keeps its lease and the
/// thread that renews it.
struct Member {
    id: String,
    node: Node,
    lease: Arc<Membership>,
    stop: Arc<AtomicBool>,
    timer: Option<JoinHandle<()>>,
}

impl Member {
    /// A kill or a freeze: the node's lease timer stops, as a stopped
    /// process's does. The node itself stays, so a thawed owner can write.
    fn stop_timer(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(timer) = self.timer.take() {
            timer.join().expect("timer thread");
        }
    }
}

struct Run {
    c: Cluster,
    rng: Rng,
    acked: usize,
    next_id: usize,
}

fn membership(c: &Cluster) -> Arc<Membership> {
    Arc::new(
        Membership::new(
            c.io(),
            "",
            Arc::new(SystemClock),
            LeaseConfig {
                ttl_millis: TTL,
                max_members: 64,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    )
}

fn now() -> i64 {
    SystemClock.now_millis()
}

impl Run {
    fn start(seed: u64) -> Self {
        let c = cluster();
        create(&c, "logs", "u1");
        Self {
            c,
            rng: Rng(seed),
            acked: 0,
            next_id: 0,
        }
    }

    fn member(&self, n: usize, takeover: bool) -> Member {
        let id = format!("node-{n}");
        let takeover = takeover.then(|| {
            Takeover::from_leases(
                self.c.base(),
                "",
                &id,
                i64::try_from(n).expect("term"),
                1 << 20,
            )
        });
        let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
        let node = node_with_ttl(
            &self.c,
            &id,
            n as u64,
            clock,
            Box::new(|| u64::try_from(now()).unwrap_or(0)),
            takeover,
            |routes| Arc::new(routes),
            (
                Arc::new(DescriptorIncarnation::new(Arc::clone(&self.c.descriptors))),
                Duration::from_secs(600),
            ),
            None,
            TTL,
        );
        let mut merge = MergeConfig::off();
        merge.enabled = true;
        node.role.set_merging(
            &merge,
            &MemoryBudget::new(
                &BudgetConfig::new(256 << 20)
                    .with_pool(Pool::OpenIndexes, 128 << 20)
                    .expect("fits"),
            ),
        );
        node.keeper
            .renew(&node.committer)
            .expect("first lease written");
        // A real timer thread renews beside the writes, as a node's does.
        let stop = Arc::new(AtomicBool::new(false));
        let timer = {
            let stop = Arc::clone(&stop);
            let keeper = Arc::clone(&node.keeper);
            let committer = Arc::clone(&node.committer);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(STEP as u64));
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    keeper
                        .renew(&committer)
                        .expect("lease renewed and log trimmed");
                }
            })
        };
        Member {
            id,
            node,
            lease: membership(&self.c),
            stop,
            timer: Some(timer),
        }
    }

    /// Writes one document through `m`; the ledger takes it only if it came
    /// back written, and its sequence number is the next of the shard.
    fn write(&mut self, m: &Member) -> bool {
        let id = format!("d{}", self.next_id);
        match write_in(&m.node.role, "logs", &id) {
            ItemResult::Written { written, .. } => {
                assert_eq!(
                    usize::try_from(written.seq_no).expect("seq"),
                    self.acked,
                    "{id}: a document acknowledged, lost or counted twice"
                );
                self.acked += 1;
                self.next_id += 1;
                true
            }
            _ => false,
        }
    }

    /// `millis` of a live owner: writes now and then, publishes now and then,
    /// while its timer thread renews.
    fn live(&mut self, m: &Member, millis: i64) {
        let until = Instant::now() + Duration::from_millis(millis as u64);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(STEP as u64 / 2));
            if self.rng.below(2) == 0 {
                for _ in 0..=self.rng.below(3) {
                    assert!(self.write(m), "a live owner's write is refused");
                }
            }
            if self.rng.below(12) == 0 {
                m.node.role.refresh(&info(), 0).expect("published");
            }
        }
    }

    /// The owner has stopped; `next` takes the shard. Returns the real
    /// milliseconds from the stop to the first acknowledgement.
    fn take_over(&mut self, next: &Member) -> i64 {
        let stopped = Instant::now();
        loop {
            if self.write(next) {
                return i64::try_from(stopped.elapsed().as_millis()).expect("millis");
            }
            assert!(
                stopped.elapsed() < Duration::from_millis(10 * TTL as u64),
                "no take-over in ten leases"
            );
            std::thread::sleep(Duration::from_millis(STEP as u64));
        }
    }
}

/// A pass by `m` over the logs of the dead nodes, on the real clock.
fn collector(run: &Run, m: &Member, term: i64) -> DeadLogPass {
    let lease_clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let heads = Arc::new(
        oss_cluster::heads::ShardHeadStore::new(
            run.c.base(),
            "",
            lease_clock,
            TTL,
            Arc::new(oss_cluster::LivenessOracle::new(Arc::clone(&m.lease))),
        )
        .expect("heads"),
    );
    DeadLogPass::new(
        DeadLogParts {
            membership: Arc::clone(&m.lease),
            heads,
            descriptors: Arc::clone(&run.c.descriptors),
            store: run.c.base(),
            base: String::new(),
            node_id: m.id.clone(),
            term,
            budget: MemoryBudget::new(
                &BudgetConfig::new(256 << 20)
                    .with_pool(Pool::RequestBodies, 64 << 20)
                    .expect("pool"),
            ),
            buffer_bytes: 1 << 16,
            first_wait_millis: FIRST_WAIT,
            predecessors: None,
        },
        Box::new(now),
    )
}

fn log_keys(run: &Run, node: &str) -> usize {
    let mut n = 0;
    // LIST: a test counting what is left of a dead node's log after its collection.
    run.c
        .store
        .list_prefix(&format!("nodelog/{node}/"), None, 100_000, &mut |_, _| {
            n += 1
        })
        .expect("lists");
    n
}

struct Report {
    cycles: usize,
    acked: usize,
    takeovers: Vec<i64>,
    frozen_refused: usize,
    collected: usize,
}

/// `cycles` of `live_millis` each: a live owner, a kill or a freeze, a
/// take-over by the next node and a collection of the dead one's log.
fn soak(seed: u64, cycles: usize, live_millis: i64) -> Report {
    let mut run = Run::start(seed);
    let mut owner = run.member(1, false);
    let mut report = Report {
        cycles,
        acked: 0,
        takeovers: Vec::new(),
        frozen_refused: 0,
        collected: 0,
    };
    for n in 2..cycles + 2 {
        run.live(&owner, live_millis);
        let frozen = run.rng.below(3) == 0;
        owner.stop_timer();
        let mut next = run.member(n, true);
        report.takeovers.push(run.take_over(&next));
        if frozen {
            // The old owner thaws and writes: the take-over fenced it.
            let id = format!("d{}", run.next_id);
            let result = write_in(&owner.node.role, "logs", &id);
            assert!(
                !matches!(result, ItemResult::Written { .. }),
                "a thawed owner wrote past its successor"
            );
            report.frozen_refused += 1;
        }
        next.node.role.refresh(&info(), 0).expect("published");
        let pass = collector(&run, &next, i64::try_from(n).expect("term"));
        let dead = owner.id.clone();
        let mut left = log_keys(&run, &dead);
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(500));
            let refreshed = next.lease.refresh().expect("refreshes");
            let snapshot = DeadSnapshot {
                live: refreshed.live,
                dead: refreshed.dead_with_epochs,
            };
            report.collected += pass.run(&snapshot, false).collected;
            left = log_keys(&run, &dead);
            if left == 0 {
                break;
            }
        }
        assert_eq!(left, 0, "{dead}'s log outlived its collection");
        // The live owner is not collected: its next cycle writes through the
        // log the pass left it.
        std::mem::swap(&mut owner, &mut next);
        next.stop_timer();
    }
    owner.stop_timer();
    report.acked = run.acked;
    report
}

fn judge(r: &Report, wall: Duration) {
    let slowest = r.takeovers.iter().max().copied().unwrap_or(0);
    let fastest = r.takeovers.iter().min().copied().unwrap_or(0);
    eprintln!(
        "{} cycles, {} acknowledged, take-over {fastest}..{slowest} ms real (lease {TTL} ms), {} thawed owners refused, {} logs collected, {wall:?} wall",
        r.cycles, r.acked, r.frozen_refused, r.collected
    );
    // AC-1 and AC-3: nothing lost or twice (checked at every write), every
    // dead log collected.
    assert_eq!(r.collected, r.cycles, "a dead log was not collected");
    // AC-2, the other side: a take-over before the owner's lease could run
    // out means a live owner was judged dead (its last renewal is at most
    // one timer step before the stop).
    assert!(
        fastest >= TTL - 2 * STEP,
        "a take-over after {fastest} ms, before a lease of {TTL} ms could run out: {:?}",
        r.takeovers
    );
    // AC-2: within a lease and two polls, plus what a loaded runner adds.
    assert!(
        slowest <= TTL + 2 * STEP + SLACK,
        "take-overs {:?} ms against a lease of {TTL} ms",
        r.takeovers
    );
}

/// A short run for every push: two cycles.
#[test]
fn two_real_time_cycles_lose_nothing_and_collect_every_dead_log() {
    let wall = Instant::now();
    let r = soak(0x5eed, 2, 2_500);
    assert!(r.acked > 0);
    judge(&r, wall.elapsed());
}

/// The smoke run of the spec: about a minute, with a frozen owner in it.
#[test]
#[ignore = "slow"]
fn a_minute_of_real_time_kills_and_freezes_loses_nothing() {
    let wall = Instant::now();
    let r = soak(0x5eed, 10, 3_000);
    assert!(r.frozen_refused > 0, "no frozen owner thawed in this seed");
    judge(&r, wall.elapsed());
}
