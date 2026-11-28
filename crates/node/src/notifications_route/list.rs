//! Reading configs: one by id, a page of them, and the channel list, all
//! from the catalogue so the requests do not grow with the number of configs.

use std::collections::HashSet;

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::config::is_channel;
use super::{
    CATALOGUE, NotificationsService, bad, entries_of, failed, not_found, shown, summary, upsert,
};
use crate::route_util::refuse;

/// The most configs a page holds, and the most ids a request names.
const MAX_PAGE: usize = 100;

const SORT_FIELDS: [&str; 4] = [
    "name",
    "config_type",
    "last_updated_time_ms",
    "created_time_ms",
];

/// The ids a request names, by `config_id` and `config_id_list`.
pub(super) fn ids_of(req: &Request<'_>) -> Result<Vec<String>, RestError> {
    let mut ids: Vec<String> = Vec::new();
    for key in ["config_id", "config_id_list"] {
        if let Some(v) = req.param(key) {
            ids.extend(v.split(',').filter(|s| !s.is_empty()).map(str::to_owned));
        }
    }
    if ids.len() > MAX_PAGE {
        return Err(bad(format!("at most {MAX_PAGE} config ids a request")));
    }
    Ok(ids)
}

struct Query {
    from: usize,
    max: usize,
    field: &'static str,
    descending: bool,
    types: Vec<String>,
    enabled: Option<bool>,
    name: Option<String>,
    description: Option<String>,
    text: Option<String>,
}

/// A paging number; the plugin's own words for one out of range.
fn number(
    req: &Request<'_>,
    name: &str,
    default: usize,
    least: i64,
    refused: &str,
) -> Result<usize, RestError> {
    let Some(v) = req.param(name) else {
        return Ok(default);
    };
    let n: i64 = v
        .parse()
        .map_err(|_| bad(format!("[{name}] must be a number")))?;
    if n < least {
        return Err(refuse(
            400,
            "action_request_validation_exception",
            // ALLOC: error path; one message.
            format!("Validation Failed: 1: {refused};"),
        ));
    }
    Ok(usize::try_from(n).unwrap_or(default))
}

/// The words of `s` as a standard analyzer cuts them: runs of letters and
/// digits, lower-cased.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn query(req: &Request<'_>) -> Result<Query, RestError> {
    let field = match req.param("sort_field") {
        None => "last_updated_time_ms",
        Some(v) => SORT_FIELDS
            .iter()
            .find(|f| **f == &*v)
            .copied()
            .ok_or_else(|| {
                // ALLOC: error path; one message.
                refuse(
                    406,
                    "status_exception",
                    format!("Sort on {v} not acceptable"),
                )
            })?,
    };
    let descending = match req.param("sort_order").as_deref().map(str::to_lowercase) {
        None => false,
        Some(o) if o == "asc" => false,
        Some(o) if o == "desc" => true,
        Some(other) => {
            return Err(bad(format!(
                "No enum constant org.opensearch.search.sort.SortOrder.{}",
                other.to_uppercase()
            )));
        }
    };
    let enabled = match req.param("is_enabled").as_deref() {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return Err(bad("[is_enabled] must be true or false")),
    };
    let text = |k: &str| req.param(k).map(std::borrow::Cow::into_owned);
    Ok(Query {
        from: number(req, "from_index", 0, 0, "fromIndex is -ve")?,
        max: number(req, "max_items", MAX_PAGE, 1, "maxItems is not +ve")?.min(MAX_PAGE),
        field,
        descending,
        types: text("config_type")
            .map(|t| t.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        enabled,
        name: text("name"),
        description: text("description"),
        text: text("text_query").map(|t| t.to_lowercase()),
    })
}

fn matches(e: &Value, q: &Query, channels_only: bool) -> bool {
    let ty = e["config_type"].as_str().unwrap_or_default();
    let s = |k: &str| e[k].as_str().unwrap_or_default();
    (!channels_only || is_channel(ty))
        && (q.types.is_empty() || q.types.iter().any(|t| t == ty))
        && q.enabled
            .is_none_or(|b| e["is_enabled"].as_bool() == Some(b))
        && q.name.as_deref().is_none_or(|n| s("name") == n)
        && q.description.as_deref().is_none_or(|d| {
            let have = words(s("description"));
            words(d).iter().any(|w| have.contains(w))
        })
        && q.text.as_deref().is_none_or(|t| {
            s("name").to_lowercase().contains(t) || s("description").to_lowercase().contains(t)
        })
}

/// The matching entries in order, how many matched, and the page asked for.
fn select(mut entries: Vec<Value>, q: &Query, channels_only: bool) -> (usize, Vec<Value>) {
    entries.retain(|e| matches(e, q, channels_only));
    entries.sort_by(|a, b| {
        let ord = if q.field.ends_with("_ms") {
            a[q.field].as_u64().cmp(&b[q.field].as_u64())
        } else {
            a[q.field].as_str().cmp(&b[q.field].as_str())
        };
        ord.then_with(|| a["ord"].as_u64().cmp(&b["ord"].as_u64()))
            .then_with(|| a["config_id"].as_str().cmp(&b["config_id"].as_str()))
    });
    if q.descending {
        entries.reverse();
    }
    let total = entries.len();
    let page = entries.into_iter().skip(q.from).take(q.max).collect();
    (total, page)
}

fn list_body(start: usize, total: usize, key: &str, items: &[Value]) -> Value {
    json!({
        "start_index": start,
        "total_hits": total,
        "total_hit_relation": "eq",
        key: items,
    })
}

impl NotificationsService {
    /// The catalogue's entries as the configs have them: read, compared with
    /// the config marker's names, and repaired when they differ.
    fn view(&self, req: &mut Request<'_>) -> Result<Vec<Value>, RestError> {
        let mut marker = self.marker(req)?;
        let names = self
            .set(DocKind::NotificationConfigs)?
            .names(&mut marker)
            .map_err(|e| failed(&e))?;
        let kind = DocKind::NotificationCatalogue;
        let mut doc = Self::doc_buffer(req, kind, 1)?;
        let mut entries = match self.set(kind)?.get(CATALOGUE, &mut marker, &mut doc) {
            Ok(Some(n)) => entries_of(doc.get(..n)).map_err(|e| failed(&e))?,
            Ok(None) => Vec::new(),
            Err(e) => return Err(failed(&e)),
        };
        let have: HashSet<&str> = entries
            .iter()
            .filter_map(|e| e["config_id"].as_str())
            .collect();
        let named: HashSet<&str> = names.iter().map(String::as_str).collect();
        let missing: Vec<String> = names
            .iter()
            .filter(|n| !have.contains(n.as_str()))
            .cloned()
            .collect();
        let extra: Vec<String> = have
            .iter()
            .filter(|h| !named.contains(**h))
            .map(|s| (*s).to_owned())
            .collect();
        if missing.is_empty() && extra.is_empty() {
            return Ok(entries);
        }
        let mut found = Vec::with_capacity(missing.len());
        let mut config_doc = Self::doc_buffer(req, DocKind::NotificationConfigs, 1)?;
        for id in &missing {
            if let Some(stored) = self.stored(id, &mut marker, &mut config_doc)? {
                found.push(summary(&stored));
            }
        }
        let repair = |entries: &mut Vec<Value>| {
            let before = entries.len();
            entries.retain(|e| {
                e["config_id"]
                    .as_str()
                    .is_none_or(|id| !extra.iter().any(|x| x == id))
            });
            let mut changed = entries.len() != before;
            for f in &found {
                changed |= upsert(entries, f);
            }
            changed
        };
        repair(&mut entries);
        self.catalogue_update(req, &mut { repair })?;
        Ok(entries)
    }

    /// `GET /configs/{id}`.
    pub(crate) fn get_one(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req, DocKind::NotificationConfigs, 1)?;
        let stored = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let body = list_body(0, 1, "config_list", &[shown(&stored)]);
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `GET /configs`: by id when ids are named, else a filtered page.
    pub(crate) fn list(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let ids = ids_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req, DocKind::NotificationConfigs, 1)?;
        let (start, total, found) = if ids.is_empty() {
            let q = query(req)?;
            let (total, page) = select(self.view(req)?, &q, false);
            let mut found = Vec::with_capacity(page.len());
            for entry in &page {
                let id = entry["config_id"].as_str().unwrap_or_default();
                if let Some(stored) = self.stored(id, &mut marker, &mut doc)? {
                    found.push(shown(&stored));
                }
            }
            (q.from, total, found)
        } else {
            let mut found = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(stored) = self.stored(id, &mut marker, &mut doc)? {
                    found.push(shown(&stored));
                }
            }
            if found.is_empty() {
                return Err(not_found(&ids.join(",")));
            }
            (0, found.len(), found)
        };
        let body = list_body(start, total, "config_list", &found);
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `GET /channels`: the configs a client sends to, from the catalogue alone.
    pub(crate) fn channels(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let q = query(req)?;
        let (total, page) = select(self.view(req)?, &q, true);
        let shown: Vec<Value> = page
            .iter()
            .map(|e| {
                json!({
                    "config_id": e["config_id"],
                    "name": e["name"],
                    "description": e["description"],
                    "config_type": e["config_type"],
                    "is_enabled": e["is_enabled"],
                })
            })
            .collect();
        let body = list_body(q.from, total, "channel_list", &shown);
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
