//! The janitor's pass over the shards a writer holds.
//!
//! A pass sweeps the held shards that need it, once, over one snapshot of
//! the live points in time. The shell's reconciler sweeps every shard on
//! every pass; here a shard is swept only when it published a commit the
//! last sweep did not see, or when a blob of it is still being watched for
//! the second sighting, so a held shard nothing happens to makes no request.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_format::ShardRef;
use oss_janitor::gc::{Collector, Config, Memory, Outcome, PointInTimePins, SnapshotPins};
use oss_janitor::orphans::{IndexPins, SnapshotSource};
use oss_mem::{MemoryBudget, Pool};
use oss_meta::PointInTimeStore;

use super::WriterRole;

/// A shard this node holds: its index's name and uuid, its number, and the
/// commit it last published.
#[derive(Debug, Clone)]
pub struct HeldShard {
    /// The index's name.
    pub name: String,
    /// The index's uuid.
    pub uuid: String,
    /// The shard number.
    pub shard: u32,
    /// The commit manifest the shard last published, if it has.
    pub newest: Option<Arc<[u8]>>,
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JanitorReport {
    /// Shards swept.
    pub swept: usize,
    /// Blobs deleted.
    pub deleted: usize,
    /// Blobs a dry run would have deleted.
    pub would_delete: usize,
    /// Shards whose sweep deleted nothing because a pin could not say what
    /// it holds.
    pub pinned: usize,
    /// Sweeps that failed, as `[index][shard]` and the reason; each shard is
    /// swept again at the next pass.
    pub failed: Vec<String>,
    /// The pass had no memory for its buffer, or could not read the points
    /// in time, and swept nothing.
    pub skipped: Option<String>,
}

/// What bounds a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassLimits {
    /// The buffer a pass reserves: it must hold the largest point in time
    /// and the largest manifest.
    pub buffer_bytes: usize,
    /// The most points in time that may be live for a pass to run.
    pub max_views: usize,
}

/// A shard a snapshot holds: the commit it was seen at and when.
type Held = (Arc<[u8]>, i64);

/// What the pass remembers between runs.
struct State {
    memory: Memory,
    // The commit each shard was last swept at, by uuid and number.
    swept: HashMap<(String, u32), Arc<[u8]>>,
    // Shards a snapshot held whole: the commit and when it was seen so. A
    // snapshot is long-lived, so such a shard is looked at again only after
    // `relist_after_millis`, not on every pass.
    held: HashMap<(String, u32), Held>,
}

/// Runs the janitor's sweeps over a [`WriterRole`]'s shards.
pub struct JanitorPass {
    store: Arc<dyn BlobStore>,
    base: String,
    role: Arc<WriterRole>,
    pits: PointInTimeStore,
    budget: MemoryBudget,
    buffer_bytes: usize,
    max_views: usize,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    state: Mutex<State>,
}

impl JanitorPass {
    /// A pass over `role`'s shards of the deployment under `base` in
    /// `store`, pinned by the points in time of `pits`. Each pass reserves
    /// one buffer of `limits.buffer_bytes` from the request-body pool of
    /// `budget`, and refuses to run when more than `limits.max_views`
    /// points in time are live.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        role: Arc<WriterRole>,
        pits: PointInTimeStore,
        budget: &MemoryBudget,
        limits: PassLimits,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            store,
            // ALLOC: the base, once a node.
            base: base.to_owned(),
            role,
            pits,
            budget: budget.clone(),
            buffer_bytes: limits.buffer_bytes,
            max_views: limits.max_views,
            clock,
            state: Mutex::new(State {
                memory: Memory::new(),
                // ALLOC: one entry a shard swept, at most the shards held.
                swept: HashMap::new(),
                // ALLOC: one entry a shard a snapshot holds, at most the shards held.
                held: HashMap::new(),
            }),
        }
    }

    /// Whether `shard` needs a sweep: it published a commit the last sweep
    /// did not see, or a blob of it is being watched.
    fn due(&self, state: &State, shard: &HeldShard, now: i64, recheck: i64) -> bool {
        let Some(newest) = &shard.newest else {
            return false;
        };
        let key = (shard.uuid.clone(), shard.shard);
        if let Some((commit, at)) = state.held.get(&key)
            && commit == newest
            && now.saturating_sub(*at) < recheck
        {
            return false;
        }
        if state.swept.get(&key) != Some(newest) {
            return true;
        }
        let at = ShardRef {
            index: &shard.name,
            uuid: &shard.uuid,
            shard: shard.shard,
        };
        at.base_path(&self.base)
            .is_ok_and(|path| state.memory.watching(&path) > 0)
    }

    /// How many shards the pass holds a swept commit for, and how many the
    /// collector's memory holds a scope for.
    pub fn remembered(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.swept.len(), state.memory.scopes())
    }

    /// Forgets what the pass knows of shards the role no longer holds: a
    /// shard held again is swept as a new owner's, with nothing carried over.
    fn evict(&self, state: &mut State, held: &[HeldShard]) {
        // ALLOC: one key a shard held, once a pass.
        let keys: HashSet<(&str, u32)> = held
            .iter()
            .map(|shard| (shard.uuid.as_str(), shard.shard))
            .collect();
        state
            .swept
            .retain(|(uuid, shard), _| keys.contains(&(uuid.as_str(), *shard)));
        state
            .held
            .retain(|(uuid, shard), _| keys.contains(&(uuid.as_str(), *shard)));
        // ALLOC: one path a shard held, once a pass.
        let paths: HashSet<String> = held
            .iter()
            .filter_map(|shard| {
                ShardRef {
                    index: &shard.name,
                    uuid: &shard.uuid,
                    shard: shard.shard,
                }
                .base_path(&self.base)
                .ok()
            })
            .collect();
        state.memory.retain(|path| paths.contains(path));
    }

    /// Runs one pass under `config`.
    pub fn run(&self, config: Config) -> JanitorReport {
        let mut report = JanitorReport::default();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = (self.clock)();
        let held = self.role.held_shards();
        self.evict(&mut state, &held);
        let due: Vec<HeldShard> = held
            .into_iter()
            .filter(|shard| self.due(&state, shard, now, config.relist_after_millis))
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
        let views =
            match PointInTimePins::snapshot(&self.pits, &self.base, self.max_views, &mut buf) {
                Ok(pins) => pins,
                Err(e) => {
                    // ALLOC: error path; the failure's words.
                    report.skipped = Some(e.to_string());
                    return report;
                }
            };
        // A snapshot holds every blob of the index incarnations it names.
        let mut snapshotted = IndexPins::new(self.max_views);
        let source = SnapshotSource::new(
            Arc::clone(&self.store),
            &self.base,
            self.max_views,
            self.buffer_bytes,
        );
        if let Err(e) = source.collect_with(&mut snapshotted, &mut buf) {
            // ALLOC: error path; the failure's words.
            report.skipped = Some(e.to_string());
            return report;
        }
        // ALLOC: one flag a due shard.
        let by_snapshot: Vec<bool> = due
            .iter()
            .map(|s| snapshotted.contains(&s.name, &s.uuid))
            .collect();
        let pins = SnapshotPins::new(&views, snapshotted);
        let collector = Collector::new(&*self.store, &self.base, &pins, &*self.clock, config);
        for (shard, by_snapshot) in due.into_iter().zip(by_snapshot) {
            let at = ShardRef {
                index: &shard.name,
                uuid: &shard.uuid,
                shard: shard.shard,
            };
            match collector.sweep(at, &mut state.memory, &mut buf) {
                Ok(sweep) => {
                    report.swept = report.swept.saturating_add(1);
                    report.deleted = report.deleted.saturating_add(sweep.deleted.len());
                    report.would_delete =
                        report.would_delete.saturating_add(sweep.would_delete.len());
                    if sweep.outcome == Outcome::Pinned {
                        report.pinned = report.pinned.saturating_add(1);
                    }
                    // A shard with no commit yet, or pinned whole by a view, is
                    // tried again; one a snapshot holds is looked at again after
                    // `relist_after_millis`.
                    match (sweep.outcome, shard.newest) {
                        (Outcome::Swept, Some(newest)) => {
                            // ALLOC: one entry a shard swept.
                            state.swept.insert((shard.uuid, shard.shard), newest);
                        }
                        (Outcome::Pinned, Some(newest)) if by_snapshot => {
                            // ALLOC: one entry a shard a snapshot holds.
                            state.held.insert((shard.uuid, shard.shard), (newest, now));
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    // ALLOC: error path; one message.
                    report
                        .failed
                        .push(format!("[{}][{}]: {e}", shard.name, shard.shard));
                }
            }
        }
        report
    }
}
