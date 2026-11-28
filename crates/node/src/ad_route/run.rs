//! One run of a detector: claim the slot, read its window, feed the model,
//! write the result.
//!
//! A run is the slot's claim (one register swap, one more request when this
//! node has no hint), one search for the window, and one call into the
//! result sink. The model lives in this node's memory. It is used only
//! while this node's claims are unbroken: each claim swaps the register
//! from the generation the last claim returned, so a swap that lands means
//! no other node claimed in between. Any other claim (a first one, a lost
//! one, a swap that had to read first) drops the model and starts a new
//! one, which scores nothing until it has seen enough windows.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use oss_anomaly::{Model, Workspace};
use oss_blob::BlobStore;
use oss_meta::{MetaError, ScheduleRegistry};
use oss_rest::RestError;
use serde_json::{Value, json};

use super::slots::{Timing, latest_due, window};
use super::{WindowSearch, preview, windows};
use oss_ext::doc_sink::{Doc, DocSink};
use oss_ext::host::HostError;

/// The registry namespace of anomaly detection.
pub(super) const NAMESPACE: &str = "ad";
/// The tenant the detectors are scheduled under.
pub(super) const TENANT: &str = "_default_";
/// The most models one node holds.
pub const MAX_HELD_MODELS: usize = 300;
/// The result index of a detector with none of its own.
const RESULT_INDEX: &str = "opensearch-ad-plugin-result";

/// What a run did.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// No slot is due that this node has not already run.
    NotDue,
    /// Another node holds the slot, or already ran it.
    Lost {
        /// The slot.
        slot: u64,
    },
    /// The detector has category fields, which a run does not serve yet.
    Unsupported,
    /// The model is at its cap on this node.
    Skipped {
        /// The slot.
        slot: u64,
    },
    /// The window held no data.
    NoData {
        /// The slot.
        slot: u64,
    },
    /// The model has not seen enough windows to score.
    Filling {
        /// The slot.
        slot: u64,
    },
    /// The window was scored and its result written.
    Scored {
        /// The slot.
        slot: u64,
        /// The anomaly grade.
        grade: f64,
    },
}

/// Why a run failed.
#[derive(Debug)]
pub enum RunError {
    /// The claim register or a document set failed.
    Meta(MetaError),
    /// The search failed.
    Search(RestError),
    /// The result sink failed.
    Sink(HostError),
    /// The index refused the result document with this status.
    Rejected(u16),
    /// The detector or the model cannot be run.
    Bad(String),
}

struct Held {
    model: Model,
    workspace: Workspace,
    /// The claim register's generation after this node's last claim.
    generation: u64,
    /// The slot of that claim.
    slot: u64,
}

/// Runs detectors on one node.
pub struct Runner {
    registry: ScheduleRegistry,
    search: Arc<dyn WindowSearch>,
    sink: Arc<dyn DocSink>,
    timing: Timing,
    held: Mutex<HashMap<String, Held>>,
}

impl Runner {
    /// A runner over the detectors under `base` in `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base the registry refuses.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        search: Arc<dyn WindowSearch>,
        sink: Arc<dyn DocSink>,
        timing: Timing,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            registry: ScheduleRegistry::in_namespace(Arc::clone(store), base, NAMESPACE)?,
            search,
            sink,
            timing,
            held: Mutex::new(HashMap::new()),
        })
    }

    /// How many models this node holds.
    pub fn held(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Runs the newest due slot of detector `id` at `now_ms`, if this node
    /// wins its claim.
    ///
    /// # Errors
    ///
    /// [`RunError`] when the claim, the search or the sink fails; the slot
    /// stays claimed and is not retried.
    pub fn run(&self, id: &str, detector: &Value, now_ms: i64) -> Result<Outcome, RunError> {
        let period = windows::interval_ms(detector).map_err(RunError::Bad)?;
        let Some(slot) = latest_due(now_ms, period, self.timing) else {
            return Ok(Outcome::NotDue);
        };
        if !windows::categories(detector).is_empty() {
            return Ok(Outcome::Unsupported);
        }
        let (hint, last) = {
            let held = self.lock();
            let state = held.get(id);
            (state.map(|h| h.generation), state.map(|h| h.slot))
        };
        // A hinted claim swaps without reading, so a slot this node already
        // claimed would be claimed again: it is not asked for, and the
        // newest slot being the one already run, nothing is due.
        if last.is_some_and(|last| slot <= last) {
            return Ok(Outcome::NotDue);
        }
        let claim = self
            .registry
            .claim(TENANT, id, slot, hint)
            .map_err(RunError::Meta)?;
        if !claim.won {
            self.lock().remove(id);
            return Ok(Outcome::Lost { slot });
        }
        // The swap from the hinted generation moves it by one; any other
        // landing means a claim by another node came between.
        let unbroken = hint.is_some_and(|h| h.checked_add(1) == Some(claim.generation));
        let mut held = self.take(id, unbroken);
        let delay = windows::delay_ms(detector);
        let (from, to) = window(slot, period, delay)
            .ok_or_else(|| RunError::Bad("the window leaves the time range".into()))?;
        let span = windows::Span {
            from_ms: from,
            interval_ms: period,
            count: 1,
        };
        let indices: Vec<String> = detector["indices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|i| i.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let request = windows::request(detector, &span).map_err(RunError::Bad)?;
        let answer = self
            .search
            .search(&indices, &request)
            .map_err(RunError::Search)?;
        let values = windows::parse(&answer, detector, &span)
            .map_err(RunError::Bad)?
            .into_iter()
            .next()
            .flatten();
        let features = windows::features(detector).map_err(RunError::Bad)?;
        if held.is_none() {
            if self.held() >= MAX_HELD_MODELS {
                return Ok(Outcome::Skipped { slot });
            }
            let params = preview::params(detector, features.len()).map_err(RunError::Bad)?;
            let workspace = Workspace::new(&params);
            let model = Model::new(params).map_err(|e| RunError::Bad(e.to_string()))?;
            held = Some(Held {
                model,
                workspace,
                generation: claim.generation,
                slot,
            });
        }
        let Some(mut state) = held else {
            return Ok(Outcome::Skipped { slot });
        };
        state.generation = claim.generation;
        state.slot = slot;
        let scored = state
            .model
            .feed(&mut state.workspace, slot, values.as_deref())
            .map_err(|e| RunError::Bad(e.to_string()))?;
        self.lock().insert(id.to_owned(), state);
        let Some(values) = values else {
            return Ok(Outcome::NoData { slot });
        };
        let Some(scored) = scored else {
            return Ok(Outcome::Filling { slot });
        };
        let data: Vec<Value> = features
            .iter()
            .zip(&values)
            .map(|(f, x)| json!({"feature_id": f.id, "feature_name": f.name, "data": x}))
            .collect();
        let doc = json!({
            "detector_id": id,
            "data_start_time": from,
            "data_end_time": to,
            "execution_start_time": now_ms,
            "execution_end_time": now_ms,
            "anomaly_score": scored.score,
            "anomaly_grade": scored.grade,
            "confidence": scored.confidence,
            "feature_data": data,
            "model_id": id,
            "schema_version": 0,
        });
        let index = detector["result_index"].as_str().unwrap_or(RESULT_INDEX);
        let results = self
            .sink
            .index(
                index,
                &[Doc {
                    id: None,
                    // ALLOC: one result document's text, a few hundred bytes.
                    source: doc.to_string().into_bytes(),
                }],
            )
            .map_err(RunError::Sink)?;
        if let Some(r) = results.iter().find(|r| r.status >= 300) {
            return Err(RunError::Rejected(r.status));
        }
        Ok(Outcome::Scored {
            slot,
            grade: scored.grade,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Held>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The held model of `id`, taken out of the map when this node's claims
    /// are unbroken; a broken line drops it.
    fn take(&self, id: &str, unbroken: bool) -> Option<Held> {
        let mut held = self.lock();
        let state = held.remove(id);
        state.filter(|_| unbroken)
    }
}
