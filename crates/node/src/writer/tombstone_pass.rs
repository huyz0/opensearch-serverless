//! The pass that removes the tombstones of deleted indexes.
//!
//! A delete leaves a tombstone register and a marker under its hour; the
//! sweep in `oss-meta` removes them once the quarantine has passed, and this
//! decides who runs it and when. One node of the fleet sweeps: the live
//! ingest member that ranks first by rendezvous on a fixed name, so the
//! others make no request. It sweeps once an interval, never at boot, and a
//! sweep with nothing to remove is one LIST of the marker prefix, so the
//! fleet's cost is one request an interval whatever its size.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::{BlobStore, MAX_LIST_KEYS};
use oss_cluster::placement::reapers_for;
use oss_format::NodeLease;
use oss_mem::{MemoryBudget, Pool};
use oss_meta::{DEFAULT_QUARANTINE_MILLIS, DeleteMode, DescriptorStore, Sweep};

/// The name the sweeper's role is ranked by.
const ROLE: &str = "tombstone-sweep";

/// Finds whether the store honours a conditional delete: run once, by the
/// first sweep.
pub type DeleteProbe = Box<dyn Fn() -> DeleteMode + Send + Sync>;

/// What a pass reads and writes through.
pub struct TombstoneParts {
    /// The store the descriptors live in.
    pub store: Arc<dyn BlobStore>,
    /// The deployment's base path.
    pub base: String,
    /// The node sweeping.
    pub node_id: String,
    /// The memory the buffer comes from.
    pub budget: MemoryBudget,
    /// The buffer a sweep reads tombstones through.
    pub buffer_bytes: usize,
    /// How long after one sweep (or the pass's making) the next may run;
    /// 0 turns the pass off.
    pub every_millis: i64,
    /// How the store's conditional delete is proved.
    pub probe: DeleteProbe,
}

/// What one call came to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TombstoneReport {
    /// This node is the sweeper of the live members it was given.
    pub designated: bool,
    /// A sweep is due: the interval has passed.
    pub due: bool,
    /// A sweep ran.
    pub swept: bool,
    /// Tombstones removed.
    pub removed: u64,
    /// Markers whose name was live, gone or deleted again: dropped.
    pub stale: u64,
    /// Markers kept for a later sweep.
    pub kept: u64,
    /// Why a due sweep did not run for want of memory.
    pub skipped: Option<String>,
    /// The sweep's error, if the store failed it.
    pub failed: Option<String>,
}

struct State {
    last: i64,
    /// What the probe found, once made: the descriptors to sweep, or none
    /// when the store does not honour a conditional delete.
    descriptors: Option<Option<DescriptorStore>>,
}

/// Sweeps tombstones on behalf of one node.
pub struct TombstonePass {
    parts: TombstoneParts,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    state: Mutex<State>,
}

impl TombstonePass {
    /// A pass over `parts`, judging time by `clock`; its first sweep is one
    /// interval away.
    pub fn new(parts: TombstoneParts, clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let last = clock();
        Self {
            parts,
            clock,
            state: Mutex::new(State {
                last,
                descriptors: None,
            }),
        }
    }

    /// Sweeps if this node is the sweeper of `live` and the interval has
    /// passed.
    pub fn run(&self, live: &[NodeLease]) -> TombstoneReport {
        let mut report = TombstoneReport {
            designated: reapers_for(ROLE, live.iter(), 1).contains(&self.parts.node_id.as_str()),
            ..TombstoneReport::default()
        };
        if !report.designated || self.parts.every_millis <= 0 {
            return report;
        }
        let now = (self.clock)();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        report.due = now.saturating_sub(state.last) >= self.parts.every_millis;
        if !report.due {
            return report;
        }
        let Ok(_reserved) = self
            .parts
            .budget
            .try_reserve(Pool::RequestBodies, self.parts.buffer_bytes)
        else {
            report.skipped = Some("no memory for the sweep's buffer".to_owned());
            return report;
        };
        state.descriptors.get_or_insert_with(|| {
            let mode = (self.parts.probe)();
            (mode == DeleteMode::Conditional)
                .then(|| {
                    DescriptorStore::new(
                        Arc::clone(&self.parts.store),
                        &self.parts.base,
                        DeleteMode::Conditional,
                    )
                    .ok()
                })
                .flatten()
        });
        state.last = now;
        let Some(descriptors) = state.descriptors.as_ref().and_then(Option::as_ref) else {
            return report;
        };
        // ALLOC: one buffer a sweep, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.parts.buffer_bytes];
        let sweep = Sweep {
            now_millis: now,
            quarantine_millis: DEFAULT_QUARANTINE_MILLIS,
            page: MAX_LIST_KEYS,
            owns_bucket: &|_| true,
            clock: &*self.clock,
        };
        report.swept = true;
        match descriptors.sweep_tombstones(&sweep, &mut buf, &mut |_| {}) {
            Ok(swept) => {
                report.removed = swept.removed;
                report.stale = swept.stale;
                report.kept = swept.kept;
            }
            // ALLOC: error path; one message.
            Err(e) => report.failed = Some(e.to_string()),
        }
        report
    }
}
