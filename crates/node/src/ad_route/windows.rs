//! The reader search a detector's windows come from, and the windows read
//! back from its answer.
//!
//! A window is one detection interval of the time field: a
//! `date_histogram` of the span with each enabled feature's aggregation under
//! it. A bucket with no documents, or a feature with no value, is a missing
//! window.

use serde_json::{Value, json};

/// The windows a search covers: `count` of `interval_ms`, the first starting
/// at `from_ms` (epoch milliseconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Where the first window starts.
    pub from_ms: i64,
    /// The length of a window.
    pub interval_ms: i64,
    /// How many windows.
    pub count: usize,
}

impl Span {
    /// Where the span ends: the first millisecond after its last window.
    pub fn end_ms(&self) -> i64 {
        let count = i64::try_from(self.count).unwrap_or(i64::MAX);
        self.from_ms
            .saturating_add(self.interval_ms.saturating_mul(count))
    }

    /// Where window `slot` starts.
    pub fn start_of(&self, slot: usize) -> i64 {
        let slot = i64::try_from(slot).unwrap_or(i64::MAX);
        self.from_ms
            .saturating_add(self.interval_ms.saturating_mul(slot))
    }
}

/// One enabled feature of a detector.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    /// The feature's id, which names its aggregation in the search.
    pub id: String,
    /// The feature's name.
    pub name: String,
    /// The aggregation itself, such as `{"max": {"field": "cpu"}}`.
    pub agg: Value,
}

/// The detector's enabled features in order.
///
/// # Errors
///
/// A stored feature that is not shaped as a detector's feature is.
pub fn features(detector: &Value) -> Result<Vec<Feature>, String> {
    let list = detector["feature_attributes"]
        .as_array()
        .ok_or("the detector has no features")?;
    let mut out = Vec::with_capacity(list.len());
    for f in list.iter().filter(|f| f["feature_enabled"] == true) {
        let id = f["feature_id"].as_str().ok_or("a feature has no id")?;
        let name = f["feature_name"].as_str().unwrap_or(id);
        let agg = f["aggregation_query"]
            .as_object()
            .and_then(|m| m.values().next())
            .ok_or("a feature has no aggregation")?;
        out.push(Feature {
            id: id.to_owned(),
            name: name.to_owned(),
            agg: agg.clone(),
        });
    }
    Ok(out)
}

/// The detection interval of `detector` in milliseconds.
///
/// # Errors
///
/// A detector with no interval.
pub fn interval_ms(detector: &Value) -> Result<i64, String> {
    detector["detection_interval"]["period"]["interval"]
        .as_i64()
        .filter(|m| *m > 0)
        .map(|m| m.saturating_mul(60_000))
        .ok_or_else(|| "the detector has no detection interval".to_owned())
}

/// The search body for the windows of `span`: nothing but the histogram.
///
/// # Errors
///
/// A detector whose features cannot be read.
pub fn request(detector: &Value, span: &Span) -> Result<Value, String> {
    let time_field = detector["time_field"]
        .as_str()
        .ok_or("the detector has no time field")?;
    let mut subs = serde_json::Map::new();
    for f in features(detector)? {
        subs.insert(f.id, f.agg);
    }
    let end = span.end_ms();
    Ok(json!({
        "size": 0,
        "track_total_hits": false,
        "query": {"bool": {"filter": [
            {"range": {time_field: {"gte": span.from_ms, "lt": end}}},
            detector["filter_query"].clone(),
        ]}},
        "aggs": {"windows": {
            "date_histogram": {
                "field": time_field,
                "fixed_interval": format!("{}ms", span.interval_ms),
                "offset": format!("{}ms", span.from_ms.rem_euclid(span.interval_ms)),
                "min_doc_count": 0,
                "extended_bounds": {"min": span.from_ms, "max": end.saturating_sub(1)},
            },
            "aggs": Value::Object(subs),
        }},
    }))
}

/// The windows of `span` read back from a search answer: one entry for each,
/// `None` when the window had no data.
///
/// # Errors
///
/// An answer that does not hold the histogram.
pub fn parse(
    answer: &Value,
    detector: &Value,
    span: &Span,
) -> Result<Vec<Option<Vec<f32>>>, String> {
    let buckets = answer["aggregations"]["windows"]["buckets"]
        .as_array()
        .ok_or("the search answer has no windows")?;
    let features = features(detector)?;
    // ALLOC: one entry a window, at most the preview bound.
    let mut out: Vec<Option<Vec<f32>>> = vec![None; span.count];
    for b in buckets {
        let Some(key) = b["key"].as_i64() else {
            continue;
        };
        let offset = key.saturating_sub(span.from_ms);
        if offset < 0 || offset.checked_rem(span.interval_ms) != Some(0) {
            continue;
        }
        let Ok(slot) = usize::try_from(offset.checked_div(span.interval_ms).unwrap_or(-1)) else {
            continue;
        };
        if b["doc_count"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        // ALLOC: one vector a window with data, one value a feature.
        let values: Option<Vec<f32>> = features
            .iter()
            .map(|f| b[f.id.as_str()]["value"].as_f64().map(|v| v as f32))
            .collect();
        if let Some(entry) = out.get_mut(slot) {
            *entry = values;
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
