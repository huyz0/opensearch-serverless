//! Taking, before a write asks, the shards a dead node's write log still
//! holds records for.
//!
//! A node that dies leaves its lease naming the epochs of its log. The log's
//! last header names every shard with records not yet published, and only
//! those need work: the rest are durable in their published commits and are
//! taken lazily by their next write. Two designated reapers per dead node
//! fence the log, read the shards it names and take each through the same
//! open a write would make, so no registry of a node's shards is written or
//! listed. Nothing here is needed for correctness: a take that does not
//! happen is a lazy take later. ADR 0021.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_cluster::Refreshed;
use oss_cluster::placement::reapers_for;
use oss_format::{NodeLease, NodeLogEpoch};
use oss_types::settings::{BoolSetting, LongSetting, Scope, Setting};
use oss_writer::takeover::Predecessors;

use super::{WriterRole, lock};

/// `serverless.takeover.proactive.enabled`: take the shards a dead node's
/// log names before a write asks. Default `false`.
pub const ENABLED: BoolSetting = BoolSetting::new(
    "serverless.takeover.proactive.enabled",
    false,
    Scope::Node,
    "Take the shards a dead node's write log still holds unpublished records for as soon as \
     its lease is seen to have lapsed, instead of when a write reaches them.",
);

/// `serverless.takeover.proactive.reapers`: how many nodes act for one dead
/// node. Default 2.
pub const REAPERS: LongSetting = LongSetting::new(
    "serverless.takeover.proactive.reapers",
    2,
    1,
    16,
    Scope::Node,
    "How many live writers take a dead node's shards: the first at once, each next one a \
     refresh interval later and only for what is still not taken.",
);

/// `serverless.takeover.proactive.max_concurrent`: shards one pass takes at
/// a time. Default 4.
pub const MAX_CONCURRENT: LongSetting = LongSetting::new(
    "serverless.takeover.proactive.max_concurrent",
    4,
    1,
    64,
    Scope::Node,
    "How many shards of a dead node one pass takes at the same time, each restored into memory.",
);

/// `serverless.takeover.proactive.pass_millis`: how long a pass may start
/// takes for. Default 30,000.
pub const PASS_MILLIS: LongSetting = LongSetting::new(
    "serverless.takeover.proactive.pass_millis",
    30_000,
    1,
    3_600_000,
    Scope::Node,
    "How long a pass over one dead node starts taking shards for, in milliseconds; shards it \
     did not reach are taken by a later pass or by the next write.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    ENABLED.setting(),
    REAPERS.setting(),
    MAX_CONCURRENT.setting(),
    PASS_MILLIS.setting(),
];

/// The pass's settings, as the node read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The `ENABLED` setting.
    pub enabled: bool,
    /// The `REAPERS` setting.
    pub reapers: usize,
    /// The `MAX_CONCURRENT` setting.
    pub max_concurrent: usize,
    /// The `PASS_MILLIS` setting.
    pub pass: Duration,
    /// How long each reaper after the first waits past the lease's expiry:
    /// one refresh interval, per rank.
    pub stagger_millis: i64,
}

/// What one pass came to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Shards the log named.
    pub named: usize,
    /// Shards this node now holds because of the pass.
    pub taken: usize,
    /// Shards left alone: no index name in the header, an index since
    /// deleted or recreated, or a shard number past the index's.
    pub skipped: usize,
    /// Shards whose take failed or was not started before the deadline: a
    /// later pass or the next write takes them.
    pub deferred: usize,
}

/// The longest wait is this many times the first.
const BACKOFF_CAP: i64 = 64;

/// What became of a pass over one dead node's epochs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Attempt {
    /// Every shard the log named was taken or skipped.
    Finished(Vec<NodeLogEpoch>),
    /// The log could not be read, or a shard was not taken: tried again no
    /// sooner than `at`, after a wait that doubles at each failure.
    Retry {
        epochs: Vec<NodeLogEpoch>,
        at: i64,
        wait: i64,
    },
}

impl Attempt {
    fn blocks(&self, epochs: &[NodeLogEpoch], now_millis: i64) -> bool {
        match self {
            Self::Finished(done) => done == epochs,
            Self::Retry {
                epochs: tried, at, ..
            } => tried == epochs && now_millis < *at,
        }
    }
}

/// The proactive take of a dead node's shards.
pub struct ProactiveTakeover {
    node_id: String,
    config: Config,
    role: Arc<WriterRole>,
    predecessors: Arc<Predecessors>,
    /// What became of the last pass over each dead node, so a later refresh
    /// does not start it again at once.
    done: Mutex<HashMap<String, Attempt>>,
    running: AtomicBool,
}

impl ProactiveTakeover {
    /// A pass for node `node_id`, taking through `role`, reading each dead
    /// node's log through `predecessors` (the one the role's opener uses, so
    /// a fence is made once).
    #[must_use]
    pub fn new(
        node_id: &str,
        config: Config,
        role: Arc<WriterRole>,
        predecessors: Arc<Predecessors>,
    ) -> Self {
        Self {
            // ALLOC: one name, once a node.
            node_id: node_id.to_owned(),
            config,
            role,
            predecessors,
            // ALLOC: one entry a dead node a pass finished, bounded by the fleet.
            done: Mutex::new(HashMap::new()),
            running: AtomicBool::new(false),
        }
    }

    /// The dead nodes of `refreshed` this node acts for at `now_millis`:
    /// those it is a reaper of whose turn has come and whose epochs it has
    /// not already finished.
    #[must_use]
    pub fn due<'a>(&self, refreshed: &'a Refreshed, now_millis: i64) -> Vec<&'a NodeLease> {
        if !self.config.enabled {
            return Vec::new();
        }
        let done = lock(&self.done);
        refreshed
            .dead_with_epochs
            .iter()
            .filter(|dead| {
                let reapers =
                    reapers_for(&dead.node_id, refreshed.live.iter(), self.config.reapers);
                let Some(rank) = reapers.iter().position(|id| *id == self.node_id) else {
                    return false;
                };
                let rank = i64::try_from(rank).unwrap_or(i64::MAX);
                let turn = dead
                    .expires_at_millis
                    .saturating_add(rank.saturating_mul(self.config.stagger_millis));
                now_millis >= turn
                    && !done
                        .get(&dead.node_id)
                        .is_some_and(|attempt| attempt.blocks(&dead.node_log, now_millis))
            })
            .collect()
    }

    /// Starts a pass over each due dead node on a thread of its own, unless
    /// one is running: a refresh must return to renew the lease.
    pub fn on_refresh(self: &Arc<Self>, refreshed: &Refreshed, now_millis: i64) {
        let due: Vec<NodeLease> = self
            .due(refreshed, now_millis)
            .into_iter()
            .cloned()
            // ALLOC: the dead nodes due, at most the fleet.
            .collect();
        if due.is_empty()
            || self
                .running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("proactive-takeover".to_owned())
            .spawn(move || {
                for dead in &due {
                    me.pass_at(dead, now_millis);
                }
                me.running.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.running.store(false, Ordering::Release);
        }
    }

    /// One pass over the shards the log of `dead` names: fence it, then take
    /// each shard whose index still is the one the log names, at most
    /// `max_concurrent` at a time, starting no take past the deadline. The
    /// dead node's epochs are recorded as finished only when every named
    /// shard was taken or skipped.
    pub fn pass(&self, dead: &NodeLease) -> Report {
        self.pass_at(dead, 0)
    }

    /// As [`ProactiveTakeover::pass`], at `now_millis` on the lease clock:
    /// a pass that leaves anything untaken, or cannot read the log, is
    /// tried again no sooner than a stagger on, then twice as long each
    /// time, to 64 staggers, so a log nothing can take costs a bounded
    /// number of requests an hour.
    pub fn pass_at(&self, dead: &NodeLease, now_millis: i64) -> Report {
        let mut report = Report::default();
        let Ok(Some(log)) = self.predecessors.of(&dead.node_id) else {
            self.retry_later(dead, now_millis);
            return report;
        };
        let Ok(named) = log.named() else {
            self.retry_later(dead, now_millis);
            return report;
        };
        let deadline = Instant::now().checked_add(self.config.pass);
        // ALLOC: the shards the header names that this node can take, at most
        // the shards the dead node held unpublished.
        let mut work = Vec::new();
        for key in &named {
            report.named = report.named.saturating_add(1);
            let info = key
                .index_name
                .as_deref()
                .and_then(|name| self.role.index_info(name))
                .filter(|info| info.uuid.as_str() == &*key.index_uuid && key.shard < info.shards);
            match info {
                Some(info) => work.push((info, key.shard)),
                None => report.skipped = report.skipped.saturating_add(1),
            }
        }
        for chunk in work.chunks(self.config.max_concurrent.max(1)) {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                report.deferred = report.deferred.saturating_add(chunk.len());
                continue;
            }
            let taken: Vec<bool> = std::thread::scope(|scope| {
                // ALLOC: one handle a take in the chunk, at most max_concurrent.
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|(info, shard)| scope.spawn(move || self.role.hold(info, *shard).is_ok()))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap_or(false))
                    // ALLOC: one flag a take in the chunk.
                    .collect()
            });
            for ok in taken {
                if ok {
                    report.taken = report.taken.saturating_add(1);
                } else {
                    report.deferred = report.deferred.saturating_add(1);
                }
            }
        }
        if report.deferred == 0 {
            // ALLOC: the epochs of one dead node, once a finished pass.
            lock(&self.done).insert(
                dead.node_id.clone(),
                Attempt::Finished(dead.node_log.clone()),
            );
        } else {
            self.retry_later(dead, now_millis);
        }
        report
    }

    /// Records that the pass over `dead` left work, so the next try waits.
    fn retry_later(&self, dead: &NodeLease, now_millis: i64) {
        let base = self.config.stagger_millis.max(1);
        let mut done = lock(&self.done);
        let wait = match done.get(&dead.node_id) {
            Some(Attempt::Retry { epochs, wait, .. }) if *epochs == dead.node_log => {
                wait.saturating_mul(2).min(base.saturating_mul(BACKOFF_CAP))
            }
            _ => base,
        };
        // ALLOC: the epochs of one dead node, once a failed pass.
        done.insert(
            dead.node_id.clone(),
            Attempt::Retry {
                epochs: dead.node_log.clone(),
                at: now_millis.saturating_add(wait),
                wait,
            },
        );
    }
}
