//! `model_groups/_search`: the queries the registry answers over the set.
//!
//! No query index exists, so a search is the marker's read and then a read
//! of the documents it must look at: a match-all reads one page; an exact
//! `name.keyword` term reads the name's claim and one document; any other
//! name query reads the whole set, refused past `TOKEN_SCAN_MAX` groups.

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::{INDEX, MlService, bad};

/// The most hits one search returns.
const MAX_SIZE: usize = 100;
/// The most groups a name-token search reads: it reads each group, 2 GETs a group, so a larger
/// set is refused and the search narrowed by an exact `name.keyword` term.
const TOKEN_SCAN_MAX: usize = 200;
/// The size a search without one returns, as the plugin's.
const DEFAULT_SIZE: usize = 10;

/// What a search asks of a model group's name.
enum Filter {
    All,
    /// The name as written: `name.keyword`.
    Exact(String),
    /// The lower-cased tokens of the query and whether all of them must
    /// appear (`term`, `match_phrase`), or any (`match`).
    Tokens(Vec<String>, bool),
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The value of `{"field": "v"}` or `{"field": {"value"|"query": "v"}}`.
fn field_value<'a>(spec: &'a Value, field: &str) -> Option<&'a str> {
    let v = spec.get(field)?;
    v.as_str().or_else(|| {
        v.get("value")
            .or_else(|| v.get("query"))
            .and_then(Value::as_str)
    })
}

fn filter_of(query: &Value) -> Result<Filter, RestError> {
    let Some(map) = query.as_object() else {
        return Err(bad("[query] must be an object"));
    };
    let mut entries = map.iter();
    let (Some((kind, spec)), None) = (entries.next(), entries.next()) else {
        return if map.is_empty() {
            Ok(Filter::All)
        } else {
            Err(bad("[query] must hold exactly one query"))
        };
    };
    match kind.as_str() {
        "match_all" => Ok(Filter::All),
        "term" | "match" | "match_phrase" => {
            if kind == "term"
                && let Some(v) = field_value(spec, "name.keyword")
            {
                return Ok(Filter::Exact(v.to_owned()));
            }
            let v = field_value(spec, "name")
                .or_else(|| field_value(spec, "name.keyword"))
                .ok_or_else(|| bad(format!("[{kind}] is supported on [name] only")))?;
            Ok(Filter::Tokens(tokens(v), kind != "match"))
        }
        other => Err(bad(format!("the query [{other}] is not supported here"))),
    }
}

fn matches(filter: &Filter, group: &Map<String, Value>) -> bool {
    let name = group.get("name").and_then(Value::as_str).unwrap_or("");
    match filter {
        Filter::All => true,
        Filter::Exact(v) => name == v,
        Filter::Tokens(wanted, all) => {
            let have = tokens(name);
            if wanted.is_empty() {
                return false;
            }
            if *all {
                wanted.iter().all(|w| have.contains(w))
            } else {
                wanted.iter().any(|w| have.contains(w))
            }
        }
    }
}

/// The direction of a `name.keyword` sort (`Some(true)` for descending), none when the
/// body has no sort; any other sort is refused.
fn sort_of(body: &Map<String, Value>) -> Result<Option<bool>, RestError> {
    let Some(sort) = body.get("sort") else {
        return Ok(None);
    };
    let entry = match sort {
        Value::Array(a) if a.len() == 1 => &a[0],
        other @ Value::Object(_) => other,
        _ => return Err(bad("[sort] supports one [name.keyword] entry")),
    };
    let order = entry
        .get("name.keyword")
        .or_else(|| entry.get("name"))
        .ok_or_else(|| bad("[sort] supports [name.keyword] only"))?;
    match order
        .as_str()
        .or_else(|| order.get("order").and_then(Value::as_str))
    {
        Some("asc") => Ok(Some(false)),
        Some("desc") => Ok(Some(true)),
        _ => Err(bad("[sort] order must be asc or desc")),
    }
}

impl MlService {
    /// `GET|POST /model_groups/_search`.
    pub(crate) fn search_groups(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.search_set(DocKind::MlModelGroups, INDEX, req, resp)
    }

    /// A search over the `kind` set, hits shown under `index`. A group's exact name is
    /// answered from its claim; any other set, and any other query, reads the set.
    pub(super) fn search_set(
        &self,
        kind: DocKind,
        index: &str,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::body_or_empty(req)?;
        let filter = match body.get("query") {
            None | Some(Value::Null) => Filter::All,
            Some(q) => filter_of(q)?,
        };
        let count = |name: &str, default: usize| -> Result<usize, RestError> {
            body.get(name).map_or(Ok(default), |v| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| bad(format!("[{name}] must be a non-negative number")))
            })
        };
        let (from, size) = (count("from", 0)?, count("size", DEFAULT_SIZE)?);
        if size > MAX_SIZE {
            return Err(bad(format!("[size] must be at most {MAX_SIZE}")));
        }
        let descending = sort_of(&body)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut names = self
            .set_of(kind)?
            .names(&mut marker)
            .map_err(|e| super::failed(&e))?;
        let mut hits: Vec<(String, Map<String, Value>)> = Vec::new();
        let total;
        match &filter {
            Filter::All if descending.is_none() => {
                total = names.len();
                names.sort();
                for id in names.iter().skip(from).take(size) {
                    if let Some(g) = self.stored_in(kind, id, &mut marker, &mut doc)? {
                        hits.push((id.clone(), g));
                    }
                }
            }
            Filter::Exact(name) if kind == DocKind::MlModelGroups => {
                if let Some(id) = self.holder(name)?
                    && let Some(g) = self.stored_in(kind, &id, &mut marker, &mut doc)?
                    && matches(&filter, &g)
                {
                    hits.push((id, g));
                }
                total = hits.len();
                hits = hits.into_iter().skip(from).take(size).collect();
            }
            Filter::All | Filter::Tokens(..) | Filter::Exact(_) => {
                if names.len() > TOKEN_SCAN_MAX {
                    let advice = if kind == DocKind::MlModelGroups {
                        "search with a [name.keyword] term instead"
                    } else {
                        "delete the connectors you no longer use"
                    };
                    return Err(bad(format!(
                        "a name or sorted search reads every document and the set holds {}; \
                         {advice}",
                        names.len()
                    )));
                }
                names.sort();
                let mut found = Vec::new();
                for id in &names {
                    if let Some(g) = self.stored_in(kind, id, &mut marker, &mut doc)?
                        && matches(&filter, &g)
                    {
                        found.push((id.clone(), g));
                    }
                }
                let by_name = |g: &Map<String, Value>| {
                    g.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                if descending.is_some() {
                    found.sort_by_key(|(_, g)| by_name(g));
                }
                if descending == Some(true) {
                    found.reverse();
                }
                total = found.len();
                hits = found.into_iter().skip(from).take(size).collect();
            }
        }
        let sorted = descending.is_some();
        let hits: Vec<Value> = hits
            .into_iter()
            .map(|(id, mut g)| {
                // A connector's sealed credential never leaves the store.
                g.remove("credential");
                let mut hit = json!({
                    "_index": index, "_id": id, "_version": 1, "_seq_no": 0,
                    "_primary_term": 1, "_score": if sorted { Value::Null } else { json!(1.0) },
                    "_source": Value::Object(g.clone()),
                });
                if sorted {
                    hit["sort"] = json!([g.get("name").cloned().unwrap_or(Value::Null)]);
                }
                hit
            })
            .collect();
        let out = json!({
            "took": 1, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {
                "total": {"value": total, "relation": "eq"},
                "max_score": if hits.is_empty() || sorted { Value::Null } else { json!(1.0) },
                "hits": hits,
            },
        });
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}
