//! The loop body of a node's anomaly detection: which detectors this node
//! owns and runs, found without polling any detector.
//!
//! A tick costs nothing between refreshes. A refresh (every `refresh_ms`, or
//! when the live nodes change) reads the detector set's marker (one request:
//! a deployment with no detector pays that and nothing more) and, only when the marker moved or the owned set changed,
//! the documents of the detectors this node owns. Between refreshes a tick
//! is the clock and the cached documents: a detector's slot comes due by
//! arithmetic, and the claim and the search happen then.
//!
//! The owner of a detector is the one live node with the highest rendezvous
//! weight for its id, so a model stays on one node; when two nodes disagree
//! about who is live the claim register decides which runs the slot.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_cluster::placement::{ShardKey, owner};
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use serde_json::Value;

use super::run::{Outcome, RunError, Runner};

/// What a tick did.
#[derive(Debug, Default)]
pub struct TickReport {
    /// Whether the detector set was read.
    pub refreshed: bool,
    /// The runs that happened, by detector id; undue detectors are left out.
    pub outcomes: Vec<(String, Outcome)>,
    /// The runs that failed, by detector id.
    pub failed: Vec<(String, RunError)>,
}

struct Owned {
    id: String,
    detector: Value,
}

#[derive(Default)]
struct State {
    refreshed_at: Option<i64>,
    /// The detector set's change count when the owned set was read.
    version: Option<i64>,
    /// The live nodes the owned set was computed for.
    live: Vec<String>,
    /// Whether the detector set has any change.
    registered: bool,
    owned: Vec<Owned>,
}

/// Decides, each tick, which detectors this node runs.
pub struct Scheduler {
    runner: Runner,
    set: DocSet,
    node: String,
    refresh_ms: i64,
    buffer_bytes: usize,
    state: Mutex<State>,
}

impl Scheduler {
    /// A scheduler for node `node` over the detectors under `base`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base the detector set refuses.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        runner: Runner,
        node: &str,
        refresh_ms: i64,
        buffer_bytes: usize,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            runner,
            set: DocSet::new(Arc::clone(store), base, DocKind::AdDetectors)?,
            // ALLOC: one copy of the node id, once.
            node: node.to_owned(),
            refresh_ms,
            buffer_bytes,
            state: Mutex::new(State::default()),
        })
    }

    /// The runner this scheduler drives.
    pub fn runner(&self) -> &Runner {
        &self.runner
    }

    /// One pass at `now_ms` with `live` the nodes this node believes are
    /// live (itself included): refresh when due, then run what is due.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the detector set cannot be read.
    pub fn tick(&self, now_ms: i64, live: &[&str]) -> Result<TickReport, MetaError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut report = TickReport::default();
        let moved = state
            .live
            .iter()
            .map(String::as_str)
            .ne(live.iter().copied());
        let due = state
            .refreshed_at
            .is_none_or(|at| now_ms.saturating_sub(at) >= self.refresh_ms);
        if due || moved {
            self.refresh(&mut state, now_ms, live)?;
            report.refreshed = true;
        }
        for owned in &state.owned {
            match self.runner.run(&owned.id, &owned.detector, now_ms) {
                Ok(Outcome::NotDue) => {}
                Ok(outcome) => report.outcomes.push((owned.id.clone(), outcome)),
                Err(e) => report.failed.push((owned.id.clone(), e)),
            }
        }
        Ok(report)
    }

    fn refresh(&self, state: &mut State, now_ms: i64, live: &[&str]) -> Result<(), MetaError> {
        state.refreshed_at = Some(now_ms);
        let mut marker = vec![0u8; self.buffer_bytes];
        let version = Some(self.set.version(&mut marker)?);
        let registered = version.is_some_and(|v| v > 0);
        let same = state.registered == registered
            && state.version == version
            && state
                .live
                .iter()
                .map(String::as_str)
                .eq(live.iter().copied());
        state.registered = registered;
        // ALLOC: one id a live node, once a refresh.
        state.live = live.iter().map(|n| (*n).to_owned()).collect();
        if same {
            return Ok(());
        }
        state.version = version;
        state.owned.clear();
        if !registered {
            return Ok(());
        }
        let mut doc = vec![0u8; DocKind::AdDetectors.max_doc_bytes()];
        for name in self.set.names(&mut marker)? {
            let key = ShardKey::new(&name, 0);
            if owner(key, live.iter().copied()) != Some(self.node.as_str()) {
                continue;
            }
            let Some(n) = self.set.get(&name, &mut marker, &mut doc)? else {
                continue;
            };
            let Ok(stored) = serde_json::from_slice::<Value>(doc.get(..n).unwrap_or_default())
            else {
                continue;
            };
            if stored["job"]["enabled"].as_bool() == Some(true) {
                state.owned.push(Owned {
                    id: name,
                    detector: stored["detector"].clone(),
                });
            }
        }
        Ok(())
    }
}
