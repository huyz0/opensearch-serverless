//! A preview: the windows of a span read from the detector's indexes, a model
//! trained on them in memory and the plugin's `anomaly_result` for the
//! windows asked for. Nothing is stored and no checkpoint is kept.

use oss_anomaly::{FeedError, Model, Params, Scored, Workspace};
use serde_json::{Value, json};

use super::windows::{Feature, Span};

/// The most windows one preview reads, the history it trains on included.
pub(crate) const MAX_PREVIEW_POINTS: usize = 1_000;

/// The seed of a preview's model: the same windows give the same grades.
const SEED: u64 = 0x00AD_5EED;

/// The windows of the span asked for, and the history before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Windows before the span, read to train the model.
    pub history: usize,
    /// Every window searched: the history, then the span.
    pub all: Span,
}

/// How a preview of `windows` windows from `from_ms` is searched: the
/// history is what the model needs to score (its training windows and one
/// shingle), cut back so the whole stays within [`MAX_PREVIEW_POINTS`].
///
/// # Errors
///
/// A span of more windows than a preview reads.
pub(crate) fn plan(
    from_ms: i64,
    interval_ms: i64,
    windows: usize,
    params: &Params,
) -> Result<Plan, String> {
    if windows == 0 || windows > MAX_PREVIEW_POINTS {
        return Err(format!(
            "a preview covers 1 to {MAX_PREVIEW_POINTS} windows, the range has {windows}"
        ));
    }
    let history = params
        .min_train_samples
        .saturating_add(params.shingle_size)
        .min(MAX_PREVIEW_POINTS.saturating_sub(windows));
    let back = i64::try_from(history)
        .unwrap_or(0)
        .saturating_mul(interval_ms);
    Ok(Plan {
        history,
        all: Span {
            from_ms: from_ms.saturating_sub(back),
            interval_ms,
            count: history.saturating_add(windows),
        },
    })
}

/// The model's parameters for `detector`.
///
/// # Errors
///
/// A detector the model cannot be built for.
pub(crate) fn params(detector: &Value, features: usize) -> Result<Params, String> {
    let mut params = Params::new(features, SEED);
    params.shingle_size = detector["shingle_size"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(params.shingle_size);
    params.validate().map_err(|e| e.to_string())?;
    Ok(params)
}

/// What one window of a preview holds besides its values: its bucket in the
/// search answer, its place in the result, and the text of both.
const WINDOW_BYTES: usize = 1 << 10;
/// What one feature adds to a window.
const FEATURE_BYTES: usize = 256;

/// The bytes a preview holds: its model and workspace, and the windows of the
/// search answer, their values and the rendered result, at the most a preview
/// reads for each of `entities`.
pub(crate) fn reserved(params: &Params, entities: usize) -> usize {
    let window = params
        .features
        .saturating_mul(FEATURE_BYTES)
        .saturating_add(WINDOW_BYTES);
    params
        .reserved_bytes()
        .saturating_add(params.workspace_bytes())
        .saturating_add(
            window
                .saturating_mul(MAX_PREVIEW_POINTS)
                .saturating_mul(entities),
        )
}

/// Feeds every window in order and keeps the scores of those from `history`.
///
/// # Errors
///
/// A window value the model refuses.
pub(crate) fn score(
    params: Params,
    windows: &[Option<Vec<f32>>],
    history: usize,
) -> Result<Vec<Option<Scored>>, String> {
    let mut workspace = Workspace::new(&params);
    let mut model = Model::new(params).map_err(|e| e.to_string())?;
    // ALLOC: one entry a window, at most the preview bound.
    let mut scored = Vec::with_capacity(windows.len().saturating_sub(history));
    for (slot, values) in windows.iter().enumerate() {
        let got = model
            .feed(&mut workspace, slot as u64, values.as_deref())
            .map_err(|e: FeedError| format!("window {slot}: {e}"))?;
        if slot >= history {
            scored.push(got);
        }
    }
    Ok(scored)
}

/// The plugin's `anomaly_result` for the windows of the span: none at all
/// when the span held no data.
pub(crate) fn results(
    features: &[Feature],
    span: &Span,
    values: &[Option<Vec<f32>>],
    scored: &[Option<Scored>],
    entity: Option<&[(String, String)]>,
) -> Vec<Value> {
    if values.iter().all(Option::is_none) {
        return Vec::new();
    }
    values
        .iter()
        .zip(scored)
        .enumerate()
        .map(|(i, (v, s))| {
            let data = features.iter().enumerate().map(|(k, f)| {
                let x = v.as_ref().and_then(|v| v.get(k));
                json!({"feature_id": f.id, "feature_name": f.name, "data": x})
            });
            let (grade, score, confidence) =
                s.map_or((0.0, 0.0, 0.0), |s| (s.grade, s.score, s.confidence));
            let mut result = json!({
                "detector_id": null,
                "anomaly_grade": grade,
                "anomaly_score": score,
                "confidence": confidence,
                "feature_data": data.collect::<Vec<_>>(),
                "data_start_time": span.start_of(i),
                "data_end_time": span.start_of(i.saturating_add(1)),
            });
            if let (Some(key), Some(m)) = (entity, result.as_object_mut()) {
                let entity: Vec<Value> = key
                    .iter()
                    .map(|(name, value)| json!({"name": name, "value": value}))
                    .collect();
                m.insert("entity".to_owned(), Value::Array(entity));
            }
            result
        })
        .collect()
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
