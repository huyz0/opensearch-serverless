//! The query DSL a monitor search takes, evaluated over one monitor's
//! source: the clauses the Alerting dashboards and clients send.

use serde_json::{Map, Value};

use super::bad;
use oss_rest::RestError;

/// Whether `query` matches a monitor `source`.
///
/// # Errors
///
/// A 400 for a clause this search does not serve or a malformed one.
pub(super) fn matches(query: &Value, id: &str, source: &Value) -> Result<bool, RestError> {
    let Some(clause) = query.as_object() else {
        return Err(bad("a query is an object"));
    };
    let mut entries = clause.iter();
    let (Some((kind, body)), None) = (entries.next(), entries.next()) else {
        return Err(bad("a query holds one clause"));
    };
    match kind.as_str() {
        "match_all" => Ok(true),
        "match_none" => Ok(false),
        "ids" => Ok(body["values"]
            .as_array()
            .is_some_and(|v| v.iter().any(|x| x.as_str() == Some(id)))),
        "bool" => boolean(body, id, source),
        "term" | "match" | "match_phrase" | "prefix" | "wildcard" | "range" => {
            field_clause(kind, body, source)
        }
        "terms" => {
            let object = single(body)?;
            let values = object
                .1
                .as_array()
                .ok_or_else(|| bad("terms takes a list"))?;
            Ok(values_at(source, object.0)
                .iter()
                .any(|have| values.iter().any(|want| same(have, want))))
        }
        "exists" => {
            let field = body["field"]
                .as_str()
                .ok_or_else(|| bad("exists takes a field"))?;
            Ok(!values_at(source, field).is_empty())
        }
        other => Err(bad(format!("unknown query [{other}]"))),
    }
}

fn single(body: &Value) -> Result<(&str, &Value), RestError> {
    let object = body
        .as_object()
        .ok_or_else(|| bad("a clause takes a field"))?;
    let mut it = object.iter();
    match (it.next(), it.next()) {
        (Some((k, v)), None) => Ok((k.as_str(), v)),
        _ => Err(bad("a clause takes one field")),
    }
}

fn boolean(body: &Value, id: &str, source: &Value) -> Result<bool, RestError> {
    let list = |name: &str| -> Result<Vec<&Value>, RestError> {
        match &body[name] {
            Value::Null => Ok(Vec::new()),
            Value::Array(items) => Ok(items.iter().collect()),
            one @ Value::Object(_) => Ok(vec![one]),
            _ => Err(bad(format!("[{name}] takes a query or a list"))),
        }
    };
    for q in list("must")?.into_iter().chain(list("filter")?) {
        if !matches(q, id, source)? {
            return Ok(false);
        }
    }
    for q in list("must_not")? {
        if matches(q, id, source)? {
            return Ok(false);
        }
    }
    let should = list("should")?;
    if should.is_empty() {
        return Ok(true);
    }
    let mut hit = 0usize;
    for q in &should {
        if matches(q, id, source)? {
            hit = hit.saturating_add(1);
        }
    }
    let needed = match &body["minimum_should_match"] {
        Value::Number(n) => n.as_u64().map_or(1, |n| usize::try_from(n).unwrap_or(1)),
        _ if list("must")?.is_empty() && list("filter")?.is_empty() => 1,
        _ => 0,
    };
    Ok(hit >= needed)
}

/// `term`, `match` and the like on one field, whose value is the scalar or
/// the clause's `value`, `query`, `gte`, `lte`, `gt`, `lt`.
fn field_clause(kind: &str, body: &Value, source: &Value) -> Result<bool, RestError> {
    let (field, spec) = single(body)?;
    let spec_object: Option<&Map<String, Value>> = spec.as_object();
    let want = spec_object.map_or(spec, |o| {
        o.get("value")
            .or_else(|| o.get("query"))
            .unwrap_or(&Value::Null)
    });
    let have = values_at(source, field);
    let keyword = field.ends_with(".keyword");
    Ok(match kind {
        "term" => have.iter().any(|h| same(h, want)),
        "match" | "match_phrase" => have.iter().any(|h| match (h, want) {
            (Value::String(h), Value::String(w)) if !keyword => {
                let (h, w) = (h.to_lowercase(), w.to_lowercase());
                if kind == "match_phrase" {
                    h.contains(&w)
                } else {
                    w.split_whitespace()
                        .any(|t| h.split(|c: char| !c.is_alphanumeric()).any(|x| x == t))
                }
            }
            _ => same(h, want),
        }),
        "prefix" => have
            .iter()
            .any(|h| matches!((h, want), (Value::String(h), Value::String(w)) if h.starts_with(w.as_str()))),
        "wildcard" => have.iter().any(
            |h| matches!((h, want), (Value::String(h), Value::String(w)) if glob(w, h)),
        ),
        _ => {
            let bound = |k: &str| spec_object.and_then(|o| o.get(k));
            have.iter().any(|h| {
                let ok = |k: &str, f: fn(std::cmp::Ordering) -> bool| {
                    bound(k).is_none_or(|b| order(h, b).is_some_and(f))
                };
                ok("gte", |o| o.is_ge())
                    && ok("gt", |o| o.is_gt())
                    && ok("lte", |o| o.is_le())
                    && ok("lt", |o| o.is_lt())
            })
        }
    })
}

/// The values of `field` in a monitor's source: the path with the leading
/// `monitor.` and a trailing `.keyword` dropped, lists flattened.
fn values_at<'a>(source: &'a Value, field: &str) -> Vec<&'a Value> {
    let path = field.strip_prefix("monitor.").unwrap_or(field);
    let path = path.strip_suffix(".keyword").unwrap_or(path);
    let mut now = vec![source];
    for part in path.split('.') {
        now = now
            .into_iter()
            .flat_map(|v| match v {
                Value::Array(items) => items.iter().collect::<Vec<_>>(),
                other => vec![other],
            })
            .filter_map(|v| v.get(part))
            .collect();
    }
    now.into_iter()
        .flat_map(|v| match v {
            Value::Array(items) => items.iter().collect::<Vec<_>>(),
            Value::Null => Vec::new(),
            other => vec![other],
        })
        .collect()
}

fn same(have: &Value, want: &Value) -> bool {
    match (have, want) {
        (Value::String(h), Value::String(w)) => h == w,
        (Value::Number(h), Value::Number(w)) => h.as_f64() == w.as_f64(),
        (Value::Bool(h), Value::Bool(w)) => h == w,
        (Value::Bool(h), Value::String(w)) | (Value::String(w), Value::Bool(h)) => {
            w == if *h { "true" } else { "false" }
        }
        (Value::Number(h), Value::String(w)) | (Value::String(w), Value::Number(h)) => {
            h.to_string() == *w
        }
        _ => false,
    }
}

fn order(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// `*` and `?` over a whole value, matched on characters.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0usize, 0usize);
    // Where the last `*` was and the text position it has consumed up to.
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                pi = pi.saturating_add(1);
                star = Some((pi, ti));
            }
            Some(c) if *c == '?' || Some(c) == t.get(ti) => {
                pi = pi.saturating_add(1);
                ti = ti.saturating_add(1);
            }
            _ => match star {
                Some((after, at)) => {
                    let at = at.saturating_add(1);
                    star = Some((after, at));
                    (pi, ti) = (after, at);
                }
                None => return false,
            },
        }
    }
    p.get(pi..)
        .is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

/// The value a hit sorts by under `field`, as the plugin's index holds it.
pub(super) fn sort_value(source: &Value, field: &str) -> Option<Value> {
    values_at(source, field).first().map(|v| (*v).clone())
}

/// Orders two sort values: missing last.
pub(super) fn compare(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => order(a, b).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
