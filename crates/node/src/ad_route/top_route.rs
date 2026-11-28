//! `GET|POST /detectors/{id}/results/_topAnomalies`: the entities of a
//! detector with the worst or the most anomalies in a time range.
//!
//! The answers and every refusal follow the stock 3.10.0 recording
//! (`fixtures/stock-plugins/ad-results.json`, steps 84 to 128). The
//! anomalous result documents of the range are read from the default result
//! index in one search and grouped by their `entity` here, so the work is
//! bounded by [`MAX_DOCS`], not by the entities.

use std::collections::BTreeMap;

use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::run::RESULT_INDEX;
use super::{AdService, windows};
use crate::route_util::{read_body, refuse};

/// The most anomalous results one request groups.
const MAX_DOCS: usize = 10_000;

/// The most buckets a request may ask for.
const MAX_SIZE: i64 = 1_000;

/// The bucket count when the body names none.
const DEFAULT_SIZE: i64 = 10;

/// The most bytes of a body.
const MAX_BODY_BYTES: usize = 1 << 16;

/// The most bytes of the search answer, held until it is grouped.
const MAX_ANSWER_BYTES: usize = 8 << 20;

fn invalid(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn validation(reason: &str) -> RestError {
    refuse(
        400,
        "action_request_validation_exception",
        format!("Validation Failed: 1: {reason};"),
    )
}

/// What the body asks for.
struct Ask {
    size: i64,
    categories: Vec<String>,
    by_severity: bool,
    start: i64,
    end: i64,
    task_id: Option<String>,
}

/// A number the body holds as a number or as a numeric text.
fn number(value: &Value, wrap: fn(String) -> RestError) -> Result<i64, RestError> {
    match value {
        Value::Number(n) => n
            .as_i64()
            // ALLOC: error path; one message.
            .ok_or_else(|| wrap(format!("For input string: \"{n}\""))),
        Value::String(s) => s
            .trim()
            .parse()
            // ALLOC: error path; one message.
            .map_err(|_| wrap(format!("For input string: \"{s}\""))),
        other => Err(wrap(format!("For input string: \"{other}\""))),
    }
}

fn parse(body: &Value) -> Result<Ask, RestError> {
    let size = match body.get("size") {
        None | Some(Value::Null) => DEFAULT_SIZE,
        Some(v) => number(v, |m| refuse(400, "number_format_exception", m))?,
    };
    let time = |name: &str| match body.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => number(v, invalid).map(Some),
    };
    let (start, end) = (time("start_time_ms")?, time("end_time_ms")?);
    let (Some(start), Some(end)) = (start, end) else {
        return Err(validation(
            "Must set both start time and end time with epoch of milliseconds",
        ));
    };
    if start >= end {
        return Err(validation("Start time should be before end time"));
    }
    if size < 1 {
        return Err(invalid("Size must be a positive integer"));
    }
    if size > MAX_SIZE {
        return Err(invalid(format!("Size cannot exceed {MAX_SIZE}")));
    }
    let by_severity = match body.get("order").and_then(Value::as_str) {
        None | Some("severity") => true,
        Some("occurrence") => false,
        Some(other) => {
            return Err(invalid(format!(
                "Ordering by {other} is not a valid option"
            )));
        }
    };
    let categories = body["category_field"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(Ask {
        size,
        categories,
        by_severity,
        start,
        end,
        task_id: body["task_id"].as_str().map(str::to_owned),
    })
}

/// The buckets of `hits`, grouped by the values of `fields` in each result's
/// `entity`, best first.
fn buckets(hits: &[Value], fields: &[String], ask: &Ask) -> Vec<Value> {
    // ALLOC: one entry a distinct entity of the range, at most MAX_DOCS.
    let mut groups: BTreeMap<Vec<String>, (u64, f64)> = BTreeMap::new();
    for hit in hits {
        let source = &hit["_source"];
        let grade = source["anomaly_grade"].as_f64().unwrap_or(0.0);
        let entity = source["entity"].as_array().map_or(&[][..], Vec::as_slice);
        let key: Option<Vec<String>> = fields
            .iter()
            .map(|f| {
                entity
                    .iter()
                    .find(|e| e["name"].as_str() == Some(f))
                    .and_then(|e| e["value"].as_str().map(str::to_owned))
            })
            .collect();
        let Some(key) = key else {
            continue;
        };
        let group = groups.entry(key).or_insert((0, 0.0));
        group.0 = group.0.saturating_add(1);
        group.1 = group.1.max(grade);
    }
    let mut ordered: Vec<(Vec<String>, (u64, f64))> = groups.into_iter().collect();
    ordered.sort_by(|a, b| {
        let (x, y) = (a.1, b.1);
        let by = if ask.by_severity {
            y.1.total_cmp(&x.1).then(y.0.cmp(&x.0))
        } else {
            y.0.cmp(&x.0).then(y.1.total_cmp(&x.1))
        };
        by.then_with(|| a.0.cmp(&b.0))
    });
    ordered
        .into_iter()
        .take(usize::try_from(ask.size).unwrap_or(0))
        .map(|(key, (count, grade))| {
            let mut k = serde_json::Map::new();
            for (field, value) in fields.iter().zip(key) {
                k.insert(field.clone(), Value::String(value));
            }
            json!({"key": k, "doc_count": count, "max_anomaly_grade": grade})
        })
        .collect()
}

impl AdService {
    /// The top anomalies of detector `{id}`.
    pub(crate) fn top_anomalies(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let historical = match req.param("historical").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                return Err(invalid(format!(
                    "Failed to parse value [{other}] as only [true] or [false] are allowed."
                )));
            }
        };
        let raw = read_body(req, MAX_BODY_BYTES, "the body is over the limit")?;
        if raw.is_empty() {
            return Err(refuse(400, "parse_exception", "request body is required"));
        }
        let body: Value = serde_json::from_slice(&raw)
            .map_err(|e| refuse(400, "json_parse_exception", e.to_string()))?;
        let ask = parse(&body)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let stored = self.stored(&id, &mut marker, &mut doc)?.ok_or_else(|| {
            refuse(
                404,
                "status_exception",
                // ALLOC: error path; one message.
                format!("Can't find config with id: {id}"),
            )
        })?;
        let declared = windows::categories(&stored["detector"]);
        if declared.is_empty() {
            return Err(invalid(format!(
                "No category fields found for detector ID {id}"
            )));
        }
        if let Some(missing) = ask.categories.iter().find(|c| !declared.contains(c)) {
            return Err(invalid(format!(
                "Category field {missing} doesn't exist for detector ID {id}"
            )));
        }
        let fields = if ask.categories.is_empty() {
            declared
        } else {
            ask.categories.clone()
        };
        // A historical analysis is a task and none is run natively yet, so
        // only a task named in the body can match results: none do.
        if historical && ask.task_id.is_none() {
            return Err(refuse(
                500,
                "resource_not_found_exception",
                format!("No historical tasks found for detector ID {id}"),
            ));
        }
        let search = self.search.as_ref().ok_or_else(|| {
            refuse(
                500,
                "ad_source_unavailable",
                "this node cannot search for results",
            )
        })?;
        let mut filter = vec![
            json!({"term": {"detector_id": id}}),
            json!({"range": {"anomaly_grade": {"gt": 0}}}),
            json!({"range": {"data_end_time": {"gte": ask.start, "lte": ask.end}}}),
        ];
        match &ask.task_id {
            Some(task) => filter.push(json!({"term": {"task_id": task}})),
            None => filter.push(json!({"bool": {"must_not": {"exists": {"field": "task_id"}}}})),
        }
        let query = json!({
            "size": MAX_DOCS,
            "_source": ["anomaly_grade", "entity"],
            "query": {"bool": {"filter": filter}},
        });
        // The answer is held until it is grouped.
        req.body().reserve(MAX_ANSWER_BYTES)?;
        let answer = match search.search(&[RESULT_INDEX.to_owned()], &query) {
            Ok(a) => a,
            Err(e) if e.status() == 404 => json!({}),
            Err(e) => return Err(e),
        };
        let hits = answer["hits"]["hits"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        let out = json!({"buckets": buckets(hits, &fields, &ask)});
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}
