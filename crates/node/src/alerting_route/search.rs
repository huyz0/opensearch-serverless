//! `POST /monitors/_search`: the plugin's search over a tenant's monitors,
//! answered from the index and the bodies without a search engine.
//!
//! A search whose query is `match_all` and that sorts by nothing reads only
//! the bodies of the page it returns; any other reads every body, one request
//! each, at most [`oss_format::MAX_MONITORS`] of them, one at a time.

use std::cmp::Ordering;

use oss_format::{MAX_MONITOR_BYTES, MonitorEntry};
use oss_rest::{Exception, JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::search_query::{compare, matches, sort_value};
use super::{AlertingService, INDEX, bad, failed};
use crate::route_util::{read_body, refuse};

/// The most hits a search skips and returns together, as the index's
/// `max_result_window`.
const MAX_WINDOW: usize = 10_000;
/// The most body bytes a search holds for its page.
const MAX_RESULT_BYTES: usize = 16 << 20;
/// The body a search takes.
const MAX_REQUEST_BYTES: usize = 1 << 20;

struct Sort {
    field: String,
    descending: bool,
}

fn sorts(body: &Value) -> Result<Vec<Sort>, RestError> {
    let items = match &body["sort"] {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items.clone(),
        one => vec![one.clone()],
    };
    let mut out = Vec::new();
    for item in &items {
        let (field, order) = match item {
            Value::String(f) => (f.as_str(), None),
            Value::Object(o) if o.len() == 1 => {
                let (f, spec) = o.iter().next().ok_or_else(|| bad("a sort names a field"))?;
                let order = match spec {
                    Value::String(s) => Some(s.as_str()),
                    other => other["order"].as_str(),
                };
                (f.as_str(), order)
            }
            _ => return Err(bad("a sort is a field or an object of one")),
        };
        if field == "_score" {
            continue;
        }
        let descending = match order {
            None | Some("asc") => false,
            Some("desc") => true,
            Some(other) => return Err(bad(format!("unknown sort order [{other}]"))),
        };
        // ALLOC: one field name per sort, once a request.
        out.push(Sort {
            field: field.to_owned(),
            descending,
        });
    }
    Ok(out)
}

fn count(body: &Value, name: &str, default: usize) -> Result<usize, RestError> {
    match &body[name] {
        Value::Null => Ok(default),
        v => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| bad(format!("[{name}] must be a non-negative integer"))),
    }
}

struct Hit {
    id: String,
    entry: MonitorEntry,
    source: Value,
    keys: Vec<Option<Value>>,
    bytes: usize,
}

fn order(a: &Hit, b: &Hit, by: &[Sort]) -> Ordering {
    for (i, s) in by.iter().enumerate() {
        let c = compare(
            a.keys.get(i).and_then(Option::as_ref),
            b.keys.get(i).and_then(Option::as_ref),
        );
        let c = if s.descending { c.reverse() } else { c };
        if c != Ordering::Equal {
            return c;
        }
    }
    // Equal on every key: the order the index wrote them, as the plugin's
    // shard does.
    a.entry
        .seq_no
        .cmp(&b.entry.seq_no)
        .then_with(|| a.id.cmp(&b.id))
}

impl AlertingService {
    /// `POST|GET /monitors/_search`.
    pub(crate) fn search(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let raw = read_body(req, MAX_REQUEST_BYTES, "a search is over its size bound")?;
        if raw.iter().all(u8::is_ascii_whitespace) {
            return Err(RestError::exception(
                400,
                Exception::new(
                    "parse_exception",
                    "request body or source parameter is required",
                ),
            ));
        }
        let body: Value = serde_json::from_slice(&raw)
            .map_err(|e| RestError::json_parse(format!("invalid body: {e}")))?;
        let query = match &body["query"] {
            Value::Null => json!({"match_all": {}}),
            q => q.clone(),
        };
        let (from, size) = (count(&body, "from", 0)?, count(&body, "size", 10)?);
        let window = from.saturating_add(size);
        if window > MAX_WINDOW {
            return Err(bad(format!(
                "Result window is too large, from + size must be less than or equal to: [{MAX_WINDOW}] but was [{window}]"
            )));
        }
        let by = sorts(&body)?;
        let m = self.monitors(req)?;
        let index = m.index().map_err(|e| failed(&e))?;
        let plain = by.is_empty()
            && query["match_all"].is_object()
            && query.as_object().is_some_and(|q| q.len() == 1);
        req.body().reserve(MAX_MONITOR_BYTES)?;
        // ALLOC: one body buffer a request, MAX_MONITOR_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_MONITOR_BYTES];
        let mut total = 0usize;
        let mut held: Vec<Hit> = Vec::new();
        // ALLOC: one pair of references per monitor, at most MAX_MONITORS.
        let mut entries: Vec<(&str, &MonitorEntry)> = index.iter().collect();
        entries.sort_by(|a, b| a.1.seq_no.cmp(&b.1.seq_no).then_with(|| a.0.cmp(b.0)));
        for (position, (id, entry)) in entries.into_iter().enumerate() {
            if plain {
                total = total.saturating_add(1);
                if position < from || position >= window {
                    continue;
                }
            }
            let (entry, len) = match m.body_of(entry, &mut buf).map_err(|e| failed(&e))? {
                Some(len) => (entry.clone(), len),
                // Replaced since the index was read: the monitor as it is now.
                None => match m.get(id, &mut buf).map_err(|e| failed(&e))? {
                    Some(found) => found,
                    None => continue,
                },
            };
            let source: Value = serde_json::from_slice(buf.get(..len).unwrap_or_default())
                .map_err(|_| refuse(500, "alerting_exception", "a stored monitor is unreadable"))?;
            if !plain {
                if !matches(&query, id, &source)? {
                    continue;
                }
                total = total.saturating_add(1);
            }
            let keys = by.iter().map(|s| sort_value(&source, &s.field)).collect();
            // ALLOC: one id and one body for each hit kept, at most MAX_WINDOW and MAX_RESULT_BYTES.
            held.push(Hit {
                id: id.to_owned(),
                entry,
                source,
                keys,
                bytes: len,
            });
            if !plain && held.len() > window {
                held.sort_by(|a, b| order(a, b, &by));
                held.truncate(window);
            }
            if held.iter().map(|h| h.bytes).sum::<usize>() > MAX_RESULT_BYTES {
                return Err(bad("a search result is over its size bound"));
            }
        }
        held.sort_by(|a, b| order(a, b, &by));
        let page: Vec<Value> = held
            .iter()
            .skip(if plain { 0 } else { from })
            .take(size)
            .map(|h| {
                let mut hit = json!({
                    "_index": INDEX,
                    "_id": h.id,
                    "_version": h.entry.version,
                    "_seq_no": h.entry.seq_no,
                    "_primary_term": 1,
                    "_score": if by.is_empty() { json!(1.0) } else { Value::Null },
                    "_source": h.source,
                });
                if !by.is_empty() {
                    hit["sort"] = Value::Array(
                        h.keys
                            .iter()
                            .map(|k| k.clone().unwrap_or(Value::Null))
                            .collect(),
                    );
                }
                hit
            })
            .collect();
        let max_score = if by.is_empty() && total > 0 {
            json!(1.0)
        } else {
            Value::Null
        };
        let answer = json!({
            "took": 0,
            "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {
                "total": {"value": total, "relation": "eq"},
                "max_score": max_score,
                "hits": page,
            },
        });
        resp.send(200, JSON, answer.to_string().as_bytes())
    }
}
