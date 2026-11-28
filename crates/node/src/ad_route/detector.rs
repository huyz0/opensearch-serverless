//! A detector as a client puts it: checked against what a serverless node
//! can run, so that a feature or filter it cannot honour is refused at put
//! and never at run.

use serde_json::{Map, Value, json};

/// The most features a detector may carry (the plugin's `max_anomaly_features`).
const MAX_FEATURES: usize = 5;
/// The most `category_field`s a detector may name.
const MAX_CATEGORY_FIELDS: usize = 2;
/// The most indices a detector may read.
const MAX_INDICES: usize = 20;
/// The longest name or description, in bytes.
const MAX_NAME_BYTES: usize = 64;
const MAX_DESCRIPTION_BYTES: usize = 256;
/// The prefix a custom result index must carry.
pub(crate) const RESULT_PREFIX: &str = "opensearch-ad-plugin-result-";
/// The longest result index name.
const MAX_RESULT_INDEX_BYTES: usize = 255;
/// The metric aggregations a feature may use.
const METRICS: &[&str] = &["max", "min", "avg", "sum", "value_count", "cardinality"];
/// The query types a `filter_query` may use.
const QUERIES: &[&str] = &[
    "match_all",
    "match",
    "match_phrase",
    "multi_match",
    "term",
    "terms",
    "range",
    "exists",
    "prefix",
    "wildcard",
    "ids",
    "bool",
    "query_string",
    "simple_query_string",
    "constant_score",
];
/// What the plugin has and this node refuses at put.
const REFUSED: &[(&str, &str)] = &[
    ("rules", "suppression rules are not supported"),
    ("suppression_rules", "suppression rules are not supported"),
];
/// The units of a period, in minutes.
const UNITS: &[(&str, u64)] = &[("minutes", 1), ("hours", 60), ("days", 1_440)];

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    v.as_object()
        .ok_or_else(|| format!("[{what}] must be an object"))
}

fn text<'a>(m: &'a Map<String, Value>, key: &str, max: usize) -> Result<&'a str, String> {
    match m.get(key).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => {
            if s.len() > max {
                Err(format!("[{key}] is longer than {max} bytes"))
            } else {
                Ok(s)
            }
        }
        _ => Err(format!(
            "[{key}] is required and must be a non-empty string"
        )),
    }
}

/// A `{"period": {"interval": n, "unit": "Minutes"}}` in minutes.
fn period(v: &Value, key: &str, least: u64) -> Result<u64, String> {
    let inner = object(v, key)?
        .get("period")
        .ok_or_else(|| format!("[{key}] needs a [period]"))?;
    let inner = object(inner, "period")?;
    let interval = inner
        .get("interval")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("[{key}] needs a whole [interval]"))?;
    let unit = inner
        .get("unit")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| format!("[{key}] needs a [unit]"))?;
    let factor = UNITS
        .iter()
        .find(|(u, _)| *u == unit)
        .map(|(_, f)| *f)
        .ok_or_else(|| format!("[{key}] unit [{unit}] must be Minutes, Hours or Days"))?;
    let minutes = interval.saturating_mul(factor);
    if minutes < least {
        return Err(format!("[{key}] must be at least {least} minute(s)"));
    }
    if minutes > 7 * 1_440 {
        return Err(format!("[{key}] must be at most 7 days"));
    }
    Ok(minutes)
}

fn strings(
    m: &Map<String, Value>,
    key: &str,
    least: usize,
    most: usize,
) -> Result<Vec<String>, String> {
    let list = match m.get(key) {
        Some(Value::Array(a)) => a,
        None if least == 0 => return Ok(Vec::new()),
        _ => return Err(format!("[{key}] must be an array of strings")),
    };
    if list.len() < least || list.len() > most {
        return Err(format!(
            "[{key}] must hold between {least} and {most} entries"
        ));
    }
    list.iter()
        .map(|v| match v.as_str() {
            Some(s) if !s.is_empty() && s.len() <= 255 => Ok(s.to_owned()),
            _ => Err(format!(
                "[{key}] holds a value that is not a non-empty string"
            )),
        })
        .collect()
}

/// One feature: a name, an enabled flag and exactly one metric aggregation.
fn feature(v: &Value, ids: &mut dyn FnMut() -> Result<String, String>) -> Result<Value, String> {
    let m = object(v, "feature_attributes entry")?;
    let name = text(m, "feature_name", MAX_NAME_BYTES)?;
    let enabled = match m.get("feature_enabled") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("[feature_enabled] must be true or false".to_owned()),
    };
    let query = m
        .get("aggregation_query")
        .ok_or_else(|| format!("feature [{name}] needs an [aggregation_query]"))?;
    let aggs = object(query, "aggregation_query")?;
    if aggs.len() != 1 {
        return Err(format!(
            "feature [{name}] must have exactly one aggregation, found {}",
            aggs.len()
        ));
    }
    for (agg_name, body) in aggs {
        let body = object(body, agg_name)?;
        if body.len() != 1 {
            return Err(format!(
                "feature [{name}] aggregation [{agg_name}] must be one metric aggregation"
            ));
        }
        for (kind, spec) in body {
            if !METRICS.contains(&kind.as_str()) {
                return Err(format!(
                    "feature [{name}] aggregation type [{kind}] is not supported; use one of {}",
                    METRICS.join(", ")
                ));
            }
            let field_ok = spec
                .get("field")
                .and_then(Value::as_str)
                .is_some_and(|f| !f.is_empty());
            if !field_ok {
                return Err(format!("feature [{name}] aggregation needs a [field]"));
            }
        }
    }
    let id = match m.get("feature_id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() && id.len() <= 64 => id.to_owned(),
        _ => ids()?,
    };
    Ok(json!({
        "feature_id": id,
        "feature_name": name,
        "feature_enabled": enabled,
        "aggregation_query": query,
    }))
}

fn filter_query(v: &Value) -> Result<(), String> {
    let m = object(v, "filter_query")?;
    if m.len() != 1 {
        return Err("[filter_query] must be one query".to_owned());
    }
    for kind in m.keys() {
        if !QUERIES.contains(&kind.as_str()) {
            return Err(format!(
                "[filter_query] query type [{kind}] is not supported"
            ));
        }
    }
    Ok(())
}

/// The detector as it is stored: validated, with defaults filled in. `ids`
/// supplies a fresh feature id; `now` is the update time in milliseconds.
///
/// # Errors
///
/// The reason a field is refused, as the plugin words a 400.
pub(crate) fn checked(
    body: &Value,
    now: u64,
    ids: &mut dyn FnMut() -> Result<String, String>,
) -> Result<Value, String> {
    let m = object(body, "detector")?;
    for (key, why) in REFUSED {
        if m.contains_key(*key) {
            return Err(format!("[{key}] {why}"));
        }
    }
    let name = text(m, "name", MAX_NAME_BYTES)?;
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
    {
        return Err(
            "[name] may hold letters, digits, space, underscore, hyphen and period".to_owned(),
        );
    }
    let time_field = text(m, "time_field", 255)?;
    let indices = strings(m, "indices", 1, MAX_INDICES)?;
    let interval = period(
        m.get("detection_interval")
            .ok_or("[detection_interval] is required")?,
        "detection_interval",
        1,
    )?;
    let delay = match m.get("window_delay") {
        Some(v) => period(v, "window_delay", 0)?,
        None => 0,
    };
    let shingle = match m.get("shingle_size") {
        None => 8,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=60).contains(n))
            .ok_or("[shingle_size] must be a whole number from 1 to 60")?,
    };
    let features = m
        .get("feature_attributes")
        .and_then(Value::as_array)
        .ok_or("[feature_attributes] must be an array of features")?;
    if features.is_empty() || features.len() > MAX_FEATURES {
        return Err(format!(
            "a detector must have between 1 and {MAX_FEATURES} features, found {}",
            features.len()
        ));
    }
    let features = features
        .iter()
        .map(|f| feature(f, ids))
        .collect::<Result<Vec<_>, _>>()?;
    let names: Vec<&str> = features
        .iter()
        .filter_map(|f| f["feature_name"].as_str())
        .collect();
    if names
        .iter()
        .enumerate()
        .any(|(i, n)| names.iter().skip(i.saturating_add(1)).any(|o| o == n))
    {
        return Err("feature names must be unique".to_owned());
    }
    if features.iter().all(|f| f["feature_enabled"] == false) {
        return Err("at least one feature must be enabled".to_owned());
    }
    let filter = match m.get("filter_query") {
        None => json!({"match_all": {}}),
        Some(v) => {
            filter_query(v)?;
            v.clone()
        }
    };
    let category = strings(m, "category_field", 0, MAX_CATEGORY_FIELDS)?;
    let mut out = Map::new();
    out.insert("name".into(), json!(name));
    if let Some(d) = m.get("description").and_then(Value::as_str) {
        if d.len() > MAX_DESCRIPTION_BYTES {
            return Err(format!(
                "[description] is longer than {MAX_DESCRIPTION_BYTES} bytes"
            ));
        }
        out.insert("description".into(), json!(d));
    }
    out.insert("time_field".into(), json!(time_field));
    out.insert("indices".into(), json!(indices));
    out.insert("filter_query".into(), filter);
    out.insert(
        "detection_interval".into(),
        json!({"period": {"interval": interval, "unit": "Minutes"}}),
    );
    out.insert(
        "window_delay".into(),
        json!({"period": {"interval": delay, "unit": "Minutes"}}),
    );
    out.insert("shingle_size".into(), json!(shingle));
    out.insert("schema_version".into(), json!(0));
    out.insert("feature_attributes".into(), Value::Array(features));
    if let Some(ui) = m.get("ui_metadata") {
        out.insert("ui_metadata".into(), ui.clone());
    }
    if let Some(r) = m.get("result_index") {
        let r = r
            .as_str()
            .filter(|r| r.starts_with(RESULT_PREFIX) && r.len() <= MAX_RESULT_INDEX_BYTES)
            .filter(|r| {
                r.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
                })
            })
            .ok_or_else(|| {
                format!("[result_index] must start with [{RESULT_PREFIX}] and be lowercase")
            })?;
        out.insert("result_index".into(), json!(r));
    }
    if !category.is_empty() {
        out.insert("category_field".into(), json!(category));
        out.insert("detector_type".into(), json!("MULTI_ENTITY"));
    } else {
        out.insert("detector_type".into(), json!("SINGLE_ENTITY"));
    }
    out.insert("last_update_time".into(), json!(now));
    Ok(Value::Object(out))
}
