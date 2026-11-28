//! `GET /object`: the objects of a deployment, filtered, sorted and paged
//! as the plugin lists them.
//!
//! The marker names no summaries, so a list reads each object once to
//! filter and sort it; what a request keeps of those reads is bounded.

use std::cmp::Ordering;

use oss_meta::DocSet;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{MAX_IDS, ObservabilityService, bad, failed, object, status};
use crate::route_util::refuse;

/// The query parameters a list accepts.
pub(super) const LIST_PARAMS: &[&str] = &[
    "objectId",
    "objectIdList",
    "objectType",
    "sortField",
    "sortOrder",
    "fromIndex",
    "maxItems",
    "name",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

/// The most objects a list answers.
const MAX_ITEMS: usize = 100;

/// What a list request holds of the documents it read, so a page of small
/// objects needs no second read.
const KEEP_BYTES: usize = 4 << 20;

/// A comma separated list as its non-empty parts.
pub(super) fn split(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // ALLOC: one id each, at most the request's length.
        .map(str::to_owned)
        .collect()
}

/// A stored document as the plugin answers it.
pub(super) fn shape(id: &str, stored: &Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("objectId".to_owned(), json!(id));
    out.insert(
        "lastUpdatedTimeMs".to_owned(),
        stored["lastUpdatedTimeMs"].clone(),
    );
    out.insert("createdTimeMs".to_owned(), stored["createdTimeMs"].clone());
    out.insert("tenant".to_owned(), json!(""));
    if let Some((ty, body)) = object::type_of(stored) {
        out.insert(ty.to_owned(), Value::Object(body.clone()));
    }
    Value::Object(out)
}

/// What a list sorts and filters by, per object.
struct Summary {
    id: String,
    created: i64,
    updated: i64,
    ty: String,
    name: Option<String>,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Updated,
    Created,
    Name,
    Type,
}

struct Query {
    from: usize,
    max: usize,
    field: Field,
    descending: bool,
    types: Option<Vec<String>>,
    ids: Option<Vec<String>>,
    name: Option<String>,
}

fn validation(problems: &[&str]) -> RestError {
    let joined: String = problems
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}: {p};", i.saturating_add(1)))
        .collect();
    refuse(
        400,
        "action_request_validation_exception",
        format!("Validation Failed: {joined}"),
    )
}

fn number(req: &Request<'_>, name: &str, default: i64) -> Result<i64, RestError> {
    match req.param(name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| {
            bad(format!(
                "Failed to parse int parameter [{name}] with value [{v}]"
            ))
        }),
    }
}

fn query(req: &Request<'_>) -> Result<Query, RestError> {
    let from = number(req, "fromIndex", 0)?;
    let max = number(req, "maxItems", i64::try_from(MAX_ITEMS).unwrap_or(100))?;
    let mut problems = Vec::new();
    if from < 0 {
        problems.push("fromIndex is -ve");
    }
    if max <= 0 {
        problems.push("maxItems is not +ve");
    }
    if !problems.is_empty() {
        return Err(validation(&problems));
    }
    let field = match req.param("sortField").as_deref() {
        None | Some("lastUpdatedTimeMs") => Field::Updated,
        Some("createdTimeMs") => Field::Created,
        Some("name") => Field::Name,
        Some("objectType") => Field::Type,
        Some(other) => return Err(status(406, format!("Field {other} not acceptable"))),
    };
    let descending = match req.param("sortOrder").as_deref() {
        None => false,
        Some(o) if o.eq_ignore_ascii_case("asc") => false,
        Some(o) if o.eq_ignore_ascii_case("desc") => true,
        Some(o) => {
            return Err(bad(format!(
                "No enum constant org.opensearch.search.sort.SortOrder.{}",
                o.to_uppercase()
            )));
        }
    };
    Ok(Query {
        from: usize::try_from(from).unwrap_or(0),
        max: usize::try_from(max).unwrap_or(MAX_ITEMS).min(MAX_ITEMS),
        field,
        descending,
        types: req.param("objectType").map(|t| split(&t)),
        ids: req.param("objectIdList").map(|l| split(&l)),
        name: req.param("name").map(std::borrow::Cow::into_owned),
    })
}

fn order(a: &Summary, b: &Summary, field: Field) -> Ordering {
    let by = match field {
        Field::Updated => a.updated.cmp(&b.updated),
        Field::Created => a.created.cmp(&b.created),
        Field::Name => match (&a.name, &b.name) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
        Field::Type => a.ty.cmp(&b.ty),
    };
    by.then(a.created.cmp(&b.created))
        .then_with(|| a.id.cmp(&b.id))
}

impl ObservabilityService {
    /// `GET /object`.
    pub(crate) fn list(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let q = query(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        req.body().reserve(KEEP_BYTES)?;
        let set = self.set()?;
        if let Some(ids) = &q.ids {
            check_present(&set, ids, &mut marker)?;
        }
        let mut found: Vec<Summary> = Vec::new();
        let mut kept = 0usize;
        set.for_each(&mut marker, &mut doc, &mut |id, bytes| {
            if q.ids
                .as_ref()
                .is_some_and(|ids| !ids.iter().any(|i| i == id))
            {
                return;
            }
            let Ok(stored) = serde_json::from_slice::<Value>(bytes) else {
                return;
            };
            let Some((ty, body)) = object::type_of(&stored) else {
                return;
            };
            let name = body.get("name").and_then(Value::as_str).map(str::to_owned);
            let empty = body.is_empty();
            if q.types
                .as_ref()
                .is_some_and(|t| empty || !t.iter().any(|x| x == ty))
            {
                return;
            }
            if q.name.as_ref().is_some_and(|n| name.as_ref() != Some(n)) {
                return;
            }
            let keep = kept.saturating_add(bytes.len()) <= KEEP_BYTES;
            if keep {
                kept = kept.saturating_add(bytes.len());
            }
            // ALLOC: one summary each, at most MAX_DOCS; bytes kept up to KEEP_BYTES.
            found.push(Summary {
                id: id.to_owned(),
                created: stored["createdTimeMs"].as_i64().unwrap_or(0),
                updated: stored["lastUpdatedTimeMs"].as_i64().unwrap_or(0),
                ty: ty.to_owned(),
                name,
                bytes: keep.then(|| bytes.to_vec()),
            });
        })
        .map_err(|e| failed(&e))?;
        found.sort_by(|a, b| {
            let o = order(a, b, q.field);
            if q.descending { o.reverse() } else { o }
        });
        let total = found.len();
        let mut page = Vec::new();
        for s in found.iter_mut().skip(q.from).take(q.max) {
            let stored: Value = match s.bytes.take() {
                Some(b) => serde_json::from_slice(&b).unwrap_or(Value::Null),
                None => Self::read(&set, &s.id, &mut marker, &mut doc)?.unwrap_or(Value::Null),
            };
            if !stored.is_null() {
                page.push(shape(&s.id, &stored));
            }
        }
        let body = json!({
            "startIndex": q.from,
            "totalHits": total,
            "totalHitRelation": "eq",
            "observabilityObjectList": page,
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}

/// 404 naming the ids that the set does not hold.
fn check_present(set: &DocSet, ids: &[String], marker: &mut [u8]) -> Result<(), RestError> {
    if ids.len() > MAX_IDS {
        return Err(bad(format!("at most {MAX_IDS} object ids a request")));
    }
    let present = set.names(marker).map_err(|e| failed(&e))?;
    let missing: Vec<&str> = ids
        .iter()
        .filter(|i| {
            present
                .binary_search_by(|p| p.as_str().cmp(i.as_str()))
                .is_err()
        })
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(status(
        404,
        // ALLOC: error path; one message.
        format!("ObservabilityObject [{}] not found", missing.join(", ")),
    ))
}
