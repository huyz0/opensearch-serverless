//! A small search engine over the recorded documents: enough of the query DSL to run
//! the bodies the plan lowers to and compare their rows with stock's.

use serde_json::{Value, json};
use std::cmp::Ordering;

/// The values a field path holds in a document (arrays flatten, `.keyword` is the field).
fn values<'a>(doc: &'a Value, path: &str) -> Vec<&'a Value> {
    let path = path.strip_suffix(".keyword").unwrap_or(path);
    let mut cur = vec![doc];
    for step in path.split('.') {
        cur = cur
            .into_iter()
            .filter_map(|v| v.get(step))
            .flat_map(|v| match v {
                Value::Array(a) => a.iter().collect(),
                o => vec![o],
            })
            .collect();
    }
    cur.into_iter().filter(|v| !v.is_null()).collect()
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// `2024-01-05T10:00:00Z` as epoch milliseconds.
fn date_millis(s: &str) -> Option<i64> {
    let (d, t) = s.trim_end_matches('Z').split_once('T')?;
    let n: Vec<i64> = d
        .split('-')
        .chain(t.split(':'))
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    Some(days(n[0], n[1], n[2]) * 86_400_000 + n[3] * 3_600_000 + n[4] * 60_000 + n[5] * 1000)
}

fn cmp(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::String(x), Value::Number(y)) => date_millis(x)
            .map(|ms| ms as f64)
            .and_then(|ms| ms.partial_cmp(&y.as_f64()?)),
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn glob(p: &[char], s: &[char]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some(('*', rest)) => (0..=s.len()).any(|i| glob(rest, &s[i..])),
        Some((c, rest)) => s
            .split_first()
            .is_some_and(|(x, tail)| (*c == '?' || *c == *x) && glob(rest, tail)),
    }
}

fn text_hit(doc: &Value, field: &str, query: &str, phrase: bool, all: bool) -> bool {
    let q = tokens(query);
    values(doc, field)
        .iter()
        .filter_map(|v| v.as_str())
        .any(|s| {
            let t = tokens(s);
            if phrase {
                !q.is_empty() && t.windows(q.len()).any(|w| w == q.as_slice())
            } else if all {
                q.iter().all(|x| t.contains(x))
            } else {
                q.iter().any(|x| t.contains(x))
            }
        })
}

/// Whether the document matches the query; `Err` names what the evaluator does not know.
pub fn matches(doc: &Value, q: &Value, text: &dyn Fn(&str) -> bool) -> Result<bool, String> {
    let (kind, body) = q
        .as_object()
        .and_then(|o| o.iter().next())
        .ok_or("empty query")?;
    let one = |v: &Value| matches(doc, v, text);
    Ok(match kind.as_str() {
        "bool" => {
            let list = |k: &str| -> Vec<Value> {
                match body.get(k) {
                    Some(Value::Array(a)) => a.clone(),
                    Some(o) => vec![o.clone()],
                    None => vec![],
                }
            };
            let (must, filter, should, not) = (
                list("must"),
                list("filter"),
                list("should"),
                list("must_not"),
            );
            for c in must.iter().chain(&filter) {
                if !one(c)? {
                    return Ok(false);
                }
            }
            for c in &not {
                if one(c)? {
                    return Ok(false);
                }
            }
            let min = body
                .get("minimum_should_match")
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(
                    must.is_empty() && filter.is_empty() && !should.is_empty(),
                ));
            let hit = should
                .iter()
                .filter_map(|c| one(c).ok())
                .filter(|b| *b)
                .count() as u64;
            hit >= min
        }
        "term" => {
            let (f, v) = body
                .as_object()
                .and_then(|o| o.iter().next())
                .ok_or("term")?;
            let v = v.get("value").unwrap_or(v);
            if text(f) {
                v.as_str().is_some_and(|s| {
                    values(doc, f)
                        .iter()
                        .filter_map(|x| x.as_str())
                        .any(|x| tokens(x) == [s.to_string()])
                })
            } else {
                values(doc, f)
                    .iter()
                    .any(|x| *x == v || cmp(x, v) == Some(Ordering::Equal))
            }
        }
        "terms" => {
            let (f, vs) = body
                .as_object()
                .and_then(|o| o.iter().next())
                .ok_or("terms")?;
            vs.as_array()
                .ok_or("terms")?
                .iter()
                .any(|v| values(doc, f).contains(&v))
        }
        "range" => {
            let (f, r) = body
                .as_object()
                .and_then(|o| o.iter().next())
                .ok_or("range")?;
            let r = r.as_object().ok_or("range")?;
            values(doc, f).iter().any(|x| {
                r.iter().all(|(op, b)| match (op.as_str(), cmp(x, b)) {
                    ("gt", Some(o)) => o == Ordering::Greater,
                    ("gte", Some(o)) => o != Ordering::Less,
                    ("lt", Some(o)) => o == Ordering::Less,
                    ("lte", Some(o)) => o != Ordering::Greater,
                    _ => false,
                })
            })
        }
        "exists" => !values(doc, body["field"].as_str().ok_or("exists")?).is_empty(),
        "wildcard" => {
            let (f, w) = body
                .as_object()
                .and_then(|o| o.iter().next())
                .ok_or("wildcard")?;
            let p: Vec<char> = w["wildcard"]
                .as_str()
                .ok_or("wildcard")?
                .to_lowercase()
                .chars()
                .collect();
            values(doc, f)
                .iter()
                .filter_map(|x| x.as_str())
                .any(|s| glob(&p, &s.to_lowercase().chars().collect::<Vec<_>>()))
        }
        "match" | "match_phrase" => {
            let (f, o) = body
                .as_object()
                .and_then(|o| o.iter().next())
                .ok_or("match")?;
            text_hit(
                doc,
                f,
                o["query"].as_str().ok_or("match")?,
                kind == "match_phrase",
                false,
            )
        }
        "multi_match" | "query_string" | "simple_query_string" => {
            let q = body["query"].as_str().ok_or("query")?;
            let fields: Vec<&String> = body["fields"].as_object().ok_or("fields")?.keys().collect();
            let any = |q: &str| fields.iter().any(|f| text_hit(doc, f, q, false, false));
            if kind == "multi_match" {
                any(q)
            } else {
                // `a AND b`, or terms where `+x` is required and the rest are optional.
                let parts: Vec<&str> = q.split_whitespace().collect();
                if parts.contains(&"AND") {
                    parts.iter().filter(|p| **p != "AND").all(|p| any(p))
                } else {
                    // In the simple syntax `+` is AND: one `+` makes every term required.
                    let plus = parts.iter().any(|p| p.starts_with('+'));
                    let term = |p: &&str| any(p.trim_start_matches('+'));
                    if plus {
                        parts.iter().all(term)
                    } else {
                        parts.iter().any(term)
                    }
                }
            }
        }
        other => return Err(format!("query [{other}]")),
    })
}

/// Orders documents by the body's `sort`; missing values go where `missing` says.
pub fn sort(docs: &mut [Value], keys: &[Value]) {
    docs.sort_by(|a, b| {
        for k in keys {
            let (f, o) = k.as_object().and_then(|o| o.iter().next()).unwrap();
            let (va, vb) = (
                values(a, f).first().copied().cloned(),
                values(b, f).first().copied().cloned(),
            );
            let desc = o["order"] == "desc";
            let first = o.get("missing").map_or(!desc, |m| m == "_first");
            let ord = match (&va, &vb) {
                (None, None) => Ordering::Equal,
                (None, _) => {
                    if first {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                (_, None) => {
                    if first {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                (Some(x), Some(y)) => {
                    let c = cmp(x, y).unwrap_or(Ordering::Equal);
                    if desc { c.reverse() } else { c }
                }
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
}

fn numbers(docs: &[Value], field: &str) -> Vec<f64> {
    docs.iter()
        .flat_map(|d| values(d, field))
        .filter_map(Value::as_f64)
        .collect()
}

/// The aggregations of a body over the matching documents: `value_count`, `sum`, `avg`,
/// `min`, `max` and a `composite` of `terms` sources, as the search engine answers them.
pub fn aggregate(docs: &[Value], aggs: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (name, spec) in aggs.as_object().unwrap() {
        let (kind, body) = spec
            .as_object()
            .unwrap()
            .iter()
            .find(|(k, _)| *k != "aggregations")
            .unwrap();
        let field = body.get("field").and_then(Value::as_str).unwrap_or("");
        let value = match kind.as_str() {
            "value_count" => {
                json!({"value": if field == "_index" { docs.len() } else { docs.iter().map(|d| values(d, field).len()).sum() }})
            }
            "sum" => json!({"value": numbers(docs, field).iter().sum::<f64>()}),
            "avg" => {
                let n = numbers(docs, field);
                json!({"value": if n.is_empty() { Value::Null } else { json!(n.iter().sum::<f64>() / n.len() as f64) }})
            }
            "min" => json!({"value": numbers(docs, field).into_iter().reduce(f64::min)}),
            "max" => json!({"value": numbers(docs, field).into_iter().reduce(f64::max)}),
            "composite" => composite(docs, body, spec.get("aggregations")),
            other => panic!("aggregation [{other}]"),
        };
        out.insert(name.clone(), value);
    }
    Value::Object(out)
}

fn composite(docs: &[Value], body: &Value, subs: Option<&Value>) -> Value {
    let sources: Vec<(String, String, bool)> = body["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let (k, t) = s.as_object().unwrap().iter().next().unwrap();
            (
                k.clone(),
                t["terms"]["field"].as_str().unwrap().to_owned(),
                t["terms"]["order"] == "desc",
            )
        })
        .collect();
    let key_of = |d: &Value| -> Vec<Value> {
        sources
            .iter()
            .map(|(_, f, _)| values(d, f).first().map_or(Value::Null, |v| (*v).clone()))
            .collect()
    };
    let mut groups: Vec<(Vec<Value>, Vec<Value>)> = Vec::new();
    for d in docs {
        let k = key_of(d);
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, m)) => m.push(d.clone()),
            None => groups.push((k, vec![d.clone()])),
        }
    }
    groups.sort_by(|(a, _), (b, _)| {
        for (i, (_, _, desc)) in sources.iter().enumerate() {
            let c = match (&a[i], &b[i]) {
                (Value::Null, Value::Null) => Ordering::Equal,
                (Value::Null, _) => Ordering::Less,
                (_, Value::Null) => Ordering::Greater,
                (x, y) => cmp(x, y).unwrap_or(Ordering::Equal),
            };
            let c = if *desc && !a[i].is_null() && !b[i].is_null() {
                c.reverse()
            } else {
                c
            };
            if c != Ordering::Equal {
                return c;
            }
        }
        Ordering::Equal
    });
    let after = body.get("after").and_then(Value::as_object);
    if let Some(after) = after {
        let a: Vec<Value> = sources.iter().map(|(n, _, _)| after[n].clone()).collect();
        let pos = groups
            .iter()
            .position(|(k, _)| *k == a)
            .map_or(0, |p| p + 1);
        groups.drain(..pos);
    }
    let size = body["size"].as_u64().unwrap() as usize;
    let page: Vec<_> = groups.into_iter().take(size).collect();
    let buckets: Vec<Value> = page
        .iter()
        .map(|(k, m)| {
            let key: serde_json::Map<String, Value> = sources
                .iter()
                .zip(k)
                .map(|((n, _, _), v)| (n.clone(), v.clone()))
                .collect();
            let mut b = json!({"key": key, "doc_count": m.len()});
            if let Some(s) = subs {
                b.as_object_mut()
                    .unwrap()
                    .extend(aggregate(m, s).as_object().unwrap().clone());
            }
            b
        })
        .collect();
    let mut out = json!({"buckets": buckets});
    if buckets.len() == size {
        out["after_key"] = buckets.last().unwrap()["key"].clone();
    }
    out
}
