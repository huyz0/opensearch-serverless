//! The pass that collects the write logs of dead nodes.
//!
//! The dead members that still name log epochs come from the membership
//! refresh a node already makes, so finding them costs no request. Of each,
//! only its reapers (the two live ingest nodes that rank highest by
//! rendezvous on the dead node's id, the same two everywhere) try to collect
//! it. A log that cannot be collected yet (a shard of it is unpublished, or
//! its head is still the dead node's) is not tried again at once: the wait
//! doubles, up to a cap, so a log nothing takes over costs a bounded number
//! of requests an hour, not one pass's worth every interval.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_cluster::Membership;
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::placement::reapers_for;
use oss_format::NodeLease;
use oss_janitor::node_log::{Collected, Collector};
use oss_mem::{MemoryBudget, Pool};
use oss_meta::DescriptorStore;
use oss_writer::takeover::Predecessors;

use super::{StoreCommits, StoreNodeLogs};

/// How many live nodes reap each dead node's log.
pub const REAPERS: usize = 2;

/// The longest wait is this many times the first.
const BACKOFF_CAP: i64 = 64;

/// What a membership refresh found: who is live, and who is dead and still
/// names write-log epochs.
pub struct DeadSnapshot {
    /// The live members, this node among them.
    pub live: Arc<[NodeLease]>,
    /// The dead members that still name epochs.
    pub dead: Vec<NodeLease>,
}

/// The latest snapshot, handed from the lease renewal (which must never
/// wait on a collection) to the pass's thread.
#[derive(Default)]
pub struct DeadMailbox(Mutex<Option<DeadSnapshot>>);

impl DeadMailbox {
    /// Leaves `snapshot` for the pass, replacing one it has not taken.
    pub fn post(&self, snapshot: DeadSnapshot) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(snapshot);
    }

    /// Takes the snapshot posted since the last take, if any.
    pub fn take(&self) -> Option<DeadSnapshot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeadLogReport {
    /// Dead members that name epochs.
    pub considered: usize,
    /// Of those, the ones this node reaps.
    pub mine: usize,
    /// Of mine, skipped because an earlier try said to wait.
    pub waiting: usize,
    /// Of mine, found live again or naming nothing.
    pub nothing: usize,
    /// Of mine, still holding something unpublished.
    pub still_needed: usize,
    /// Of mine, found collectable by a dry run.
    pub would_collect: usize,
    /// Of mine, deleted.
    pub collected: usize,
    /// Of mine, whose collection failed, as the node and the reason.
    pub failed: Vec<String>,
    /// The pass had no memory for its buffer and tried nothing.
    pub skipped: Option<String>,
}

/// When a log may be tried again, and how long the last wait was.
struct Wait {
    until: i64,
    last: i64,
}

/// Collects dead nodes' logs on behalf of one node.
pub struct DeadLogPass {
    membership: Arc<Membership>,
    heads: Arc<ShardHeadStore>,
    descriptors: Arc<DescriptorStore>,
    logs: StoreNodeLogs,
    commits: StoreCommits,
    node_id: String,
    budget: MemoryBudget,
    buffer_bytes: usize,
    first_wait_millis: i64,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    waits: Mutex<HashMap<String, Wait>>,
}

/// What a pass reads and writes through.
pub struct DeadLogParts {
    /// The deployment's leases.
    pub membership: Arc<Membership>,
    /// The shard heads.
    pub heads: Arc<ShardHeadStore>,
    /// The index descriptors.
    pub descriptors: Arc<DescriptorStore>,
    /// The store logs and commits are read from.
    pub store: Arc<dyn BlobStore>,
    /// The deployment's base path.
    pub base: String,
    /// The node collecting.
    pub node_id: String,
    /// The term its fences are recorded at.
    pub term: i64,
    /// The memory the buffer comes from.
    pub budget: MemoryBudget,
    /// The buffer a pass reads descriptors through.
    pub buffer_bytes: usize,
    /// How long a log that is still needed waits before the first retry.
    pub first_wait_millis: i64,
    /// The readers the node's takes share, when it takes: a log is then
    /// fenced once however it is reached.
    pub predecessors: Option<Arc<Predecessors>>,
}

impl DeadLogPass {
    /// A pass over `parts`, judging time by `clock`.
    pub fn new(parts: DeadLogParts, clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let mut logs = StoreNodeLogs::new(
            Arc::clone(&parts.store),
            &parts.base,
            &parts.node_id,
            parts.term,
        );
        if let Some(shared) = parts.predecessors {
            logs = logs.sharing(shared);
        }
        Self {
            logs,
            commits: StoreCommits::new(parts.store, &parts.base),
            membership: parts.membership,
            heads: parts.heads,
            descriptors: parts.descriptors,
            node_id: parts.node_id,
            budget: parts.budget,
            buffer_bytes: parts.buffer_bytes,
            first_wait_millis: parts.first_wait_millis.max(1),
            clock,
            // ALLOC: one entry a dead node waiting, at most the members.
            waits: Mutex::new(HashMap::new()),
        }
    }

    /// Collects what this node reaps of `snapshot`; a dry run finds what it
    /// would and deletes nothing.
    pub fn run(&self, snapshot: &DeadSnapshot, dry_run: bool) -> DeadLogReport {
        let mut report = DeadLogReport {
            considered: snapshot.dead.len(),
            ..DeadLogReport::default()
        };
        let mut waits = self.waits.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: the ids of the dead members, at most the members.
        let named: HashSet<&str> = snapshot.dead.iter().map(|l| l.node_id.as_str()).collect();
        waits.retain(|id, _| named.contains(id.as_str()));
        let mine: Vec<&NodeLease> = snapshot
            .dead
            .iter()
            .filter(|lease| {
                reapers_for(&lease.node_id, snapshot.live.iter(), REAPERS)
                    .contains(&self.node_id.as_str())
            })
            .collect();
        report.mine = mine.len();
        let now = (self.clock)();
        let due: Vec<&NodeLease> = mine
            .into_iter()
            .filter(|lease| {
                let waiting = waits.get(&lease.node_id).is_some_and(|w| w.until > now);
                report.waiting = report.waiting.saturating_add(usize::from(waiting));
                !waiting
            })
            .collect();
        if due.is_empty() {
            return report;
        }
        let Ok(_reserved) = self
            .budget
            .try_reserve(Pool::RequestBodies, self.buffer_bytes)
        else {
            report.skipped = Some("no memory for the pass's buffer".to_owned());
            return report;
        };
        // ALLOC: one buffer a pass, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let collector = Collector::new(
            &self.membership,
            &self.heads,
            &self.descriptors,
            &self.logs,
            &self.commits,
            &*self.clock,
            dry_run,
        );
        for lease in due {
            let id = lease.node_id.as_str();
            match collector.collect(id, &mut buf) {
                Ok(Collected::Collected) => {
                    self.logs.forget(id);
                    waits.remove(id);
                    report.collected = report.collected.saturating_add(1);
                }
                Ok(Collected::Nothing) => {
                    waits.remove(id);
                    report.nothing = report.nothing.saturating_add(1);
                }
                Ok(Collected::StillNeeded) => {
                    self.wait(&mut waits, id, now);
                    report.still_needed = report.still_needed.saturating_add(1);
                }
                Ok(Collected::WouldCollect) => {
                    self.wait(&mut waits, id, now);
                    report.would_collect = report.would_collect.saturating_add(1);
                }
                Err(e) => {
                    self.wait(&mut waits, id, now);
                    // ALLOC: error path; one message.
                    report.failed.push(format!("[{id}]: {e}"));
                }
            }
        }
        report
    }

    /// Doubles `id`'s wait, from the first up to the cap.
    fn wait(&self, waits: &mut HashMap<String, Wait>, id: &str, now: i64) {
        let cap = self.first_wait_millis.saturating_mul(BACKOFF_CAP);
        let last = waits.get(id).map_or(self.first_wait_millis, |w| {
            w.last.saturating_mul(2).min(cap)
        });
        // ALLOC: one entry a dead node waiting.
        waits.insert(
            id.to_owned(),
            Wait {
                until: now.saturating_add(last),
                last,
            },
        );
    }
}
