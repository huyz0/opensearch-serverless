//! The query of `_search` over detectors. A detector's fields live in its
//! document, so a query on a field would read every detector of the tenant
//! and cost requests by documents; this serves what the marker's names can
//! answer: `match_all`, `ids` and queries on `_id`.

use serde_json::Value;

/// The most nesting a query may have.
const MAX_DEPTH: usize = 8;

fn refused(kind: &str, field: &str) -> String {
    format!(
        "query [{kind}] on [{field}] is not served: it would read every detector, so only \
         match_all, ids and queries on [_id] are"
    )
}

/// `*` and `?` matching.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => {
            (0..=text.len()).any(|i| glob(rest, text.get(i..).unwrap_or_default()))
        }
        Some((b'?', rest)) => text.split_first().is_some_and(|(_, t)| glob(rest, t)),
        Some((c, rest)) => text
            .split_first()
            .is_some_and(|(t, tail)| t == c && glob(rest, tail)),
    }
}

/// The value of a one-field clause such as `{"_id": "x"}` or `{"_id": {"value": "x"}}`.
fn one_field<'a>(kind: &str, body: &'a Value) -> Result<&'a Value, String> {
    let m = body
        .as_object()
        .filter(|m| m.len() == 1)
        .ok_or_else(|| format!("query [{kind}] needs one field"))?;
    let (field, value) = m.iter().next().ok_or("empty query")?;
    if field != "_id" {
        return Err(refused(kind, field));
    }
    Ok(value.get("value").unwrap_or(value))
}

fn clauses(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(o @ Value::Object(_)) => vec![o],
        _ => Vec::new(),
    }
}

/// Whether `query` is one this module serves, without reading a detector.
///
/// # Errors
///
/// The reason a query is not served.
pub(crate) fn check(query: &Value) -> Result<(), String> {
    matches(query, "", 0).map(|_| ())
}

/// Whether the detector `id` satisfies `query`.
///
/// # Errors
///
/// The reason a query is not served.
pub(crate) fn matches(query: &Value, id: &str, depth: usize) -> Result<bool, String> {
    if depth > MAX_DEPTH {
        return Err("the query is nested too deeply".to_owned());
    }
    let m = query
        .as_object()
        .filter(|m| m.len() == 1)
        .ok_or("a query must be one object with one query type")?;
    let (kind, body) = m.iter().next().ok_or("empty query")?;
    match kind.as_str() {
        "match_all" => Ok(true),
        "match_none" => Ok(false),
        "ids" => Ok(body
            .get("values")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(|x| x.as_str() == Some(id)))),
        "term" => Ok(one_field(kind, body)?.as_str() == Some(id)),
        "terms" => {
            let m = body.as_object().ok_or("[terms] needs an object")?;
            let list = m
                .get("_id")
                .and_then(Value::as_array)
                .ok_or_else(|| refused(kind, m.keys().next().map_or("", String::as_str)))?;
            Ok(list.iter().any(|x| x.as_str() == Some(id)))
        }
        "prefix" => Ok(one_field(kind, body)?
            .as_str()
            .is_some_and(|p| id.starts_with(p))),
        "wildcard" => Ok(one_field(kind, body)?
            .as_str()
            .is_some_and(|p| glob(p.as_bytes(), id.as_bytes()))),
        "bool" => {
            let b = body.as_object().ok_or("[bool] needs an object")?;
            for key in b.keys() {
                if ![
                    "must",
                    "filter",
                    "should",
                    "must_not",
                    "minimum_should_match",
                ]
                .contains(&key.as_str())
                {
                    return Err(format!("[bool] option [{key}] is not served"));
                }
            }
            // Every clause is evaluated, so a query that is not served is
            // refused whatever the id is.
            let all = |key: &str| -> Result<Vec<bool>, String> {
                clauses(b.get(key))
                    .into_iter()
                    .map(|c| matches(c, id, depth.saturating_add(1)))
                    .collect()
            };
            let (must, filter) = (all("must")?, all("filter")?);
            let (must_not, should) = (all("must_not")?, all("should")?);
            if !must.iter().chain(&filter).all(|&m| m) || must_not.iter().any(|&m| m) {
                return Ok(false);
            }
            let required = b.contains_key("must") || b.contains_key("filter");
            Ok(should.is_empty()
                || (required && !b.contains_key("minimum_should_match"))
                || should.iter().any(|&m| m))
        }
        other => Err(format!(
            "query [{other}] is not served: only match_all, ids, bool and queries on [_id] are"
        )),
    }
}
