//! The reader search a detector's windows come from, and the windows read
//! back from its answer.
//!
//! A window is one detection interval of the time field: a
//! `date_histogram` of the span with each enabled feature's aggregation under
//! it. A bucket with no documents, or a feature with no value, is a missing
//! window.

use std::collections::BTreeMap;

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

/// The most entities a preview of a detector with category fields shows.
pub const MAX_ENTITIES: usize = 5;

/// The name of the `terms` aggregation of category level `level`.
fn level_name(level: usize) -> String {
    format!("entity{level}")
}

/// The detector's category fields in order.
pub fn categories(detector: &Value) -> Vec<String> {
    detector["category_field"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
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

/// The delay a window leaves for late documents, in milliseconds: the
/// detector's `window_delay`, none when it has none.
#[must_use]
pub fn delay_ms(detector: &Value) -> i64 {
    detector["window_delay"]["period"]["interval"]
        .as_i64()
        .filter(|m| *m >= 0)
        .map_or(0, |m| m.saturating_mul(60_000))
}

/// The search body for the windows of `span`: nothing but the histogram.
///
/// # Errors
///
/// A detector whose features cannot be read.
pub fn request(detector: &Value, span: &Span) -> Result<Value, String> {
    request_entities(detector, span, MAX_ENTITIES)
}

/// [`request`] with up to `per_level` entities in each `terms` level.
///
/// # Errors
///
/// A detector whose features cannot be read.
pub fn request_entities(detector: &Value, span: &Span, per_level: usize) -> Result<Value, String> {
    let time_field = detector["time_field"]
        .as_str()
        .ok_or("the detector has no time field")?;
    let mut subs = serde_json::Map::new();
    for f in features(detector)? {
        subs.insert(f.id, f.agg);
    }
    // Each category field is a `terms` level above the features, the most
    // significant outermost: a window holds a handful of entities.
    let mut inner = Value::Object(subs);
    for (level, field) in categories(detector).iter().enumerate().rev() {
        inner = json!({level_name(level): {
            "terms": {"field": field, "size": per_level},
            "aggs": inner,
        }});
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
            "aggs": inner,
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

/// One entity of a detector with category fields: its category values and
/// its windows.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    /// Each category field and the entity's value in it.
    pub key: Vec<(String, String)>,
    /// How many documents the entity has over the whole span.
    pub docs: u64,
    /// The entity's windows, `None` where it had no data.
    pub windows: Vec<Option<Vec<f32>>>,
}

/// The text of a `terms` bucket's key, a string or a number.
fn key_text(key: &Value) -> String {
    key.as_str().map_or_else(|| key.to_string(), str::to_owned)
}

/// The key to an entity: each category field and its value.
type Key = Vec<(String, String)>;

/// What visits a leaf: its key and its bucket.
type Visit<'a> = dyn FnMut(&[(String, String)], &'a Value) + 'a;

/// Visits every leaf of the `terms` levels under `holder`, with the key to it.
fn leaves<'a>(
    holder: &'a Value,
    fields: &[String],
    level: usize,
    key: &mut Key,
    visit: &mut Visit<'a>,
) {
    let Some(field) = fields.get(level) else {
        visit(key, holder);
        return;
    };
    let Some(buckets) = holder[level_name(level).as_str()]["buckets"].as_array() else {
        return;
    };
    for b in buckets {
        key.push((field.clone(), key_text(&b["key"])));
        leaves(b, fields, level.saturating_add(1), key, visit);
        key.pop();
    }
}

/// The entities of `span` read back from a search answer built by
/// [`request`] for a detector with category fields: the [`MAX_ENTITIES`] with
/// the most documents, ties by key.
///
/// # Errors
///
/// An answer that does not hold the histogram.
pub fn parse_entities(
    answer: &Value,
    detector: &Value,
    span: &Span,
) -> Result<Vec<Entity>, String> {
    parse_entities_up_to(answer, detector, span, MAX_ENTITIES)
}

/// [`parse_entities`] keeping the `limit` entities with the most documents.
///
/// # Errors
///
/// An answer that does not hold the histogram.
pub fn parse_entities_up_to(
    answer: &Value,
    detector: &Value,
    span: &Span,
    limit: usize,
) -> Result<Vec<Entity>, String> {
    let buckets = answer["aggregations"]["windows"]["buckets"]
        .as_array()
        .ok_or("the search answer has no windows")?;
    let (features, fields) = (features(detector)?, categories(detector));
    let slot_of = |b: &Value| -> Option<usize> {
        b["key"]
            .as_i64()
            .map(|k| k.saturating_sub(span.from_ms))
            .filter(|o| *o >= 0 && o.checked_rem(span.interval_ms) == Some(0))
            .and_then(|o| o.checked_div(span.interval_ms))
            .and_then(|s| usize::try_from(s).ok())
            .filter(|s| *s < span.count)
    };
    // First the documents of every entity seen, which holds a key and a count
    // and no windows; then the windows of the few that are kept.
    // ALLOC: one entry a distinct entity, at most MAX_ENTITIES a level a window.
    let mut seen: BTreeMap<Vec<(String, String)>, u64> = BTreeMap::new();
    for b in buckets.iter().filter(|b| slot_of(b).is_some()) {
        leaves(b, &fields, 0, &mut Vec::new(), &mut |key, leaf| {
            let n = leaf["doc_count"].as_u64().unwrap_or(0);
            let total = seen.entry(key.to_vec()).or_insert(0);
            *total = total.saturating_add(n);
        });
    }
    let mut docs: Vec<(Vec<(String, String)>, u64)> = seen.into_iter().collect();
    docs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    docs.truncate(limit);
    let mut found: Vec<Entity> = docs
        .into_iter()
        .map(|(key, docs)| Entity {
            key,
            docs,
            // ALLOC: one entry a window, at most the preview bound, for at most MAX_ENTITIES.
            windows: vec![None; span.count],
        })
        .collect();
    for b in buckets {
        let Some(slot) = slot_of(b) else {
            continue;
        };
        leaves(b, &fields, 0, &mut Vec::new(), &mut |key, leaf| {
            let Some(entity) = found.iter_mut().find(|e| e.key == key) else {
                return;
            };
            if leaf["doc_count"].as_u64().unwrap_or(0) == 0 {
                return;
            }
            // ALLOC: one vector a window with data, one value a feature.
            let values: Option<Vec<f32>> = features
                .iter()
                .map(|f| leaf[f.id.as_str()]["value"].as_f64().map(|v| v as f32))
                .collect();
            if let Some(w) = entity.windows.get_mut(slot) {
                *w = values;
            }
        });
    }
    Ok(found)
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
