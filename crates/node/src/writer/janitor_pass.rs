//! The janitor's pass over the shards a writer holds.
//!
//! A pass sweeps the held shards that need it, once, over one snapshot of
//! the live points in time. The shell's reconciler sweeps every shard on
//! every pass; here a shard is swept only when it published a commit the
//! last sweep did not see, or when a blob of it is still being watched for
//! the second sighting, so a held shard nothing happens to makes no request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_format::ShardRef;
use oss_janitor::gc::{Collector, Config, Memory, Outcome, PointInTimePins};
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

/// What the pass remembers between runs.
struct State {
    memory: Memory,
    // The commit each shard was last swept at, by uuid and number.
    swept: HashMap<(String, u32), Arc<[u8]>>,
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
            }),
        }
    }

    /// Whether `shard` needs a sweep: it published a commit the last sweep
    /// did not see, or a blob of it is being watched.
    fn due(&self, state: &State, shard: &HeldShard) -> bool {
        let Some(newest) = &shard.newest else {
            return false;
        };
        let key = (shard.uuid.clone(), shard.shard);
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

    /// Runs one pass under `config`.
    pub fn run(&self, config: Config) -> JanitorReport {
        let mut report = JanitorReport::default();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let due: Vec<HeldShard> = self
            .role
            .held_shards()
            .into_iter()
            .filter(|shard| self.due(&state, shard))
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
        let pins = match PointInTimePins::snapshot(&self.pits, &self.base, self.max_views, &mut buf)
        {
            Ok(pins) => pins,
            Err(e) => {
                // ALLOC: error path; the failure's words.
                report.skipped = Some(e.to_string());
                return report;
            }
        };
        let collector = Collector::new(&*self.store, &self.base, &pins, &*self.clock, config);
        for shard in due {
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
                    // A shard with no commit yet, or pinned whole, is tried again.
                    if let (Outcome::Swept, Some(newest)) = (sweep.outcome, shard.newest) {
                        // ALLOC: one entry a shard swept.
                        state.swept.insert((shard.uuid, shard.shard), newest);
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
