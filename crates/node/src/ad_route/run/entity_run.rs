//! One run of a detector with category fields: the windows of its entities
//! in one search, a model for each, a result document for each that scores.

use std::collections::HashMap;
use std::sync::PoisonError;

use oss_anomaly::{Model, Workspace};
use oss_ext::doc_sink::Doc;
use serde_json::{Value, json};

use super::{EntityHeld, MAX_HELD_MODELS, Outcome, RESULT_INDEX, RunError, Runner};
use crate::ad_route::slots::window;
use crate::ad_route::{preview, windows};

/// The most entities one run reads, the most documents first: the same bound
/// is the size of each `terms` level of the search.
const MAX_RUN_ENTITIES: usize = 100;

/// The text an entity's models are held under.
fn key_text(key: &[(String, String)]) -> String {
    key.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

impl Runner {
    /// The newest due slot of a detector with category fields, claimed with
    /// `generation`: one search for every entity's window, one write for the
    /// results. An entity gets a model on its first window with data while
    /// this node has room (`MAX_HELD_MODELS` over all its detectors); one that
    /// does not is counted and left out. A model of an entity unseen for more
    /// than the model's `max_gap` windows is dropped.
    pub(super) fn run_entities(
        &self,
        id: &str,
        detector: &Value,
        (slot, generation): (u64, u64),
        unbroken: bool,
        now_ms: i64,
    ) -> Result<Outcome, RunError> {
        let period = windows::interval_ms(detector).map_err(RunError::Bad)?;
        let (from, to) = window(slot, period, windows::delay_ms(detector))
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
        let request =
            windows::request_entities(detector, &span, MAX_RUN_ENTITIES).map_err(RunError::Bad)?;
        let answer = self
            .search
            .search(&indices, &request)
            .map_err(RunError::Search)?;
        let found = windows::parse_entities_up_to(&answer, detector, &span, MAX_RUN_ENTITIES)
            .map_err(RunError::Bad)?;
        let features = windows::features(detector).map_err(RunError::Bad)?;
        let previous = self
            .entities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .filter(|_| unbroken);
        let mut models: HashMap<String, (Model, Workspace)> =
            previous.map(|h| h.models).unwrap_or_default();
        models.retain(|_, (m, _)| {
            let gap = m.params().max_gap;
            m.last_fed_slot()
                .is_none_or(|last| slot.saturating_sub(last) <= gap)
        });
        let others = self.models();
        let (mut scored, mut skipped) = (0usize, 0usize);
        let mut docs: Vec<Doc> = Vec::new();
        for entity in &found {
            let Some(values) = entity.windows.first().and_then(Option::as_ref) else {
                continue;
            };
            let key = key_text(&entity.key);
            if !models.contains_key(&key) {
                if others.saturating_add(models.len()) >= MAX_HELD_MODELS {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
                let params = preview::params(detector, features.len()).map_err(RunError::Bad)?;
                let workspace = Workspace::new(&params);
                let model = Model::new(params).map_err(|e| RunError::Bad(e.to_string()))?;
                models.insert(key.clone(), (model, workspace));
            }
            let Some((model, workspace)) = models.get_mut(&key) else {
                continue;
            };
            let Some(result) = model
                .feed(workspace, slot, Some(values))
                .map_err(|e| RunError::Bad(e.to_string()))?
            else {
                continue;
            };
            let data: Vec<Value> = features
                .iter()
                .zip(values)
                .map(|(f, x)| json!({"feature_id": f.id, "feature_name": f.name, "data": x}))
                .collect();
            let name_values: Vec<Value> = entity
                .key
                .iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect();
            let doc = json!({
                "detector_id": id,
                "data_start_time": from,
                "data_end_time": to,
                "execution_start_time": now_ms,
                "execution_end_time": now_ms,
                "anomaly_score": result.score,
                "anomaly_grade": result.grade,
                "confidence": result.confidence,
                "feature_data": data,
                "entity": name_values,
                "model_id": format!("{id}_entity_{key}"),
                "schema_version": 0,
            });
            // ALLOC: one result document's text, a few hundred bytes, for at most MAX_RUN_ENTITIES.
            docs.push(Doc {
                id: None,
                source: doc.to_string().into_bytes(),
            });
            scored = scored.saturating_add(1);
        }
        self.entities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id.to_owned(),
                EntityHeld {
                    models,
                    generation,
                    slot,
                },
            );
        if !docs.is_empty() {
            let index = detector["result_index"].as_str().unwrap_or(RESULT_INDEX);
            let results = self.sink.index(index, &docs).map_err(RunError::Sink)?;
            if let Some(r) = results.iter().find(|r| r.status >= 300) {
                return Err(RunError::Rejected(r.status));
            }
        }
        Ok(Outcome::Entities {
            slot,
            scored,
            skipped,
        })
    }
}
