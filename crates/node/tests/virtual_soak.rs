//! docs/specs/virtual-time-soak: a week of kills, freezes and take-overs on a
//! clock the test steps, over the emulator.

// ARITH: test arithmetic on counts and virtual milliseconds.
#![allow(clippy::arithmetic_side_effects)]

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use oss_blob::BlobStore as _;
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::writer::{
    DeadLogParts, DeadLogPass, DeadSnapshot, DescriptorIncarnation, MergeConfig, Takeover,
};
use oss_writer::bulk::ItemResult;
use support::*;

/// The clock every node of the run judges time by: it moves only when the
/// driver steps it, and a pause moves it rather than sleeping.
struct VirtualClock(AtomicI64);

impl VirtualClock {
    fn starting_now() -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(SystemClock.now_millis())))
    }

    fn step(&self, millis: i64) {
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

impl LeaseClock for VirtualClock {
    fn now_millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    fn pause(&self, pause: Duration) {
        self.step(i64::try_from(pause.as_millis()).unwrap_or(i64::MAX));
    }
}

#[test]
fn a_pause_moves_the_clock_and_does_not_sleep() {
    let clock = VirtualClock::starting_now();
    let before = clock.now_millis();
    let wall = std::time::Instant::now();
    clock.pause(Duration::from_secs(3_600));
    assert_eq!(clock.now_millis(), before + 3_600_000);
    assert!(wall.elapsed() < Duration::from_secs(1));
}

const TTL: i64 = 30_000;
const STEP: i64 = 5_000;
const MINUTE: i64 = 60_000;

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

/// One writer node of the run, with the handle that keeps its lease.
struct Member {
    id: String,
    node: Node,
    lease: Arc<Membership>,
}

struct Run {
    c: Cluster,
    clock: Arc<VirtualClock>,
    rng: Rng,
    /// Acknowledged documents: every one is one write that came back written.
    acked: usize,
    next_id: usize,
}

fn membership(c: &Cluster, clock: &Arc<VirtualClock>) -> Arc<Membership> {
    let clock: Arc<dyn LeaseClock> = clock.clone();
    Arc::new(
        Membership::new(
            c.io(),
            "",
            clock,
            LeaseConfig {
                ttl_millis: TTL,
                max_members: 64,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    )
}

impl Run {
    fn start(seed: u64) -> Self {
        let c = cluster();
        create(&c, "logs", "u1");
        Self {
            c,
            clock: VirtualClock::starting_now(),
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
        let clock: Arc<dyn LeaseClock> = self.clock.clone();
        let publish = Arc::clone(&self.clock);
        let node = node_with_clock(
            &self.c,
            &id,
            n as u64,
            clock,
            Box::new(move || u64::try_from(publish.now_millis()).unwrap_or(0)),
            takeover,
            |routes| Arc::new(routes),
            (
                Arc::new(DescriptorIncarnation::new(Arc::clone(&self.c.descriptors))),
                Duration::from_secs(600),
            ),
            None,
        );
        // The shipped default: a shard merges its small segments, so the
        // commit register stays bounded however many publishes a week makes.
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
        let lease = membership(&self.c, &self.clock);
        let m = Member { id, node, lease };
        self.renew(&m);
        m
    }

    /// What a live node's timer does: stamps its lease, naming its log epochs.
    fn renew(&self, m: &Member) {
        m.node
            .keeper
            .renew(&m.node.committer)
            .expect("lease renewed and log trimmed");
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

    /// `minutes` of a live owner: it renews every minute, writes a few
    /// documents now and then and publishes now and then.
    fn live(&mut self, m: &Member, minutes: i64) {
        for _ in 0..minutes {
            self.clock.step(MINUTE);
            if self.rng.below(4) == 0 {
                for _ in 0..=self.rng.below(3) {
                    assert!(self.write(m), "a live owner's write is refused");
                }
            }
            // About twice a cycle: this rig does not merge segments, and the
            // commit register it reads through a 64 KiB buffer names each.
            if self.rng.below(240) == 0 {
                m.node.role.refresh(&info(), 0).expect("published");
            }
            // After the writes, so the lease names the log as it stands.
            self.renew(m);
        }
    }

    /// The owner stops; `next` takes the shard. Returns the virtual
    /// milliseconds from the stop to the first acknowledgement.
    fn take_over(&mut self, next: &Member) -> i64 {
        let stopped = self.clock.now_millis();
        loop {
            self.clock.step(STEP);
            self.renew(next);
            if self.write(next) {
                return self.clock.now_millis() - stopped;
            }
            assert!(
                self.clock.now_millis() - stopped < 10 * TTL,
                "no take-over in ten leases"
            );
        }
    }
}

const FIRST_WAIT: i64 = 60_000;

/// A pass by `m` over the logs of the dead nodes, on the run's clock.
fn collector(run: &Run, m: &Member, term: i64) -> DeadLogPass {
    let lease_clock: Arc<dyn LeaseClock> = run.clock.clone();
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
    let clock = Arc::clone(&run.clock);
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
        Box::new(move || clock.now_millis()),
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

/// `cycles` of `minutes` each: a live owner, then a kill or a freeze, a
/// take-over by the next node, and a collection of the dead one's log.
fn soak(seed: u64, cycles: usize, minutes: i64) -> Report {
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
        run.live(&owner, minutes);
        let frozen = run.rng.below(3) == 0;
        let next = run.member(n, true);
        let took = run.take_over(&next);
        report.takeovers.push(took);
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
        for _ in 0..12 {
            run.clock.step(FIRST_WAIT * 4);
            run.renew(&next);
            let refreshed = next.lease.refresh().expect("refreshes");
            let snapshot = DeadSnapshot {
                live: refreshed.live,
                dead: refreshed.dead_with_epochs,
            };
            report.collected += pass.run(&snapshot, false).collected;
            if log_keys(&run, &dead) == 0 {
                break;
            }
        }
        assert_eq!(
            log_keys(&run, &dead),
            0,
            "{dead}'s log outlived its collection"
        );
        // The live owner is not collected: its next cycle writes and renews
        // through the log the pass left it.
        owner = next;
    }
    report.acked = run.acked;
    report
}

#[test]
fn a_week_of_kills_and_freezes_loses_nothing_and_collects_every_dead_log() {
    let wall = std::time::Instant::now();
    let r = soak(0x5eed, 21, 8 * 60);
    let slowest = r.takeovers.iter().max().copied().unwrap_or(0);
    // AC-2: the slowest of the last third is within twice the slowest of the
    // first third plus a lease: take-over time does not drift with the week.
    let third = r.takeovers.len() / 3;
    let first = r.takeovers[..third].iter().max().copied().unwrap_or(0);
    let last = r.takeovers[r.takeovers.len() - third..]
        .iter()
        .max()
        .copied()
        .unwrap_or(0);
    assert!(
        last <= 2 * first + TTL,
        "drift: first {first} ms, last {last} ms"
    );
    // AC-1: every cycle ended in a kill or a freeze, and some owners were
    // frozen and came back; each was refused.
    assert_eq!(r.cycles, 21);
    assert!(r.frozen_refused > 0, "no frozen owner thawed in this seed");
    assert_eq!(r.collected, r.cycles, "a dead log was not collected");
    eprintln!(
        "{} cycles, {} acknowledged, slowest take-over {slowest} ms, {} thawed owners refused, {} logs collected, {:?} wall",
        r.cycles,
        r.acked,
        r.frozen_refused,
        r.collected,
        wall.elapsed()
    );
    assert!(slowest <= TTL + 2 * STEP, "{:?}", r.takeovers);
}
