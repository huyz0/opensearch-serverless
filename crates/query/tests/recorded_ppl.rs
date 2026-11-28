//! The recorded stock PPL corpus (`fixtures/sql`, stock 3.10.0): every statement gives
//! stock's rows and schema over the same documents, or is refused with stock's error, or
//! is listed in `LATER` and still refused.

// ARITH: the evaluator runs over six small documents; the fixtures are a few small
// recorded files, read once.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/eval.rs"]
mod eval;

use oss_query::{Format, Limits, Mapping, parse_ppl, plan, rows, write};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

/// Statements this slice refuses, each served by a later slice or by the node.
const LATER: &[&str] = &["ppl-dedup", "ppl-parse"];

/// Statements about index names: the node resolves the names, so they are checked there.
const NODE: &[&str] = &["ppl-multi-source", "ppl-unknown-index"];

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql")
}

fn read(p: PathBuf) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        _ => a == b,
    }
}

fn corpus() -> Vec<(String, String)> {
    let text = fs::read_to_string(dir().join("corpus.txt")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut p = l.splitn(3, '|');
            match (p.next(), p.next(), p.next()) {
                (Some(n), Some("ppl"), Some(s)) => Some((n.to_owned(), s.to_owned())),
                _ => None,
            }
        })
        .collect()
}

/// Our schema and rows for a statement over the recorded documents.
fn run(
    stmt: &str,
    docs: &[Value],
    mapping: &Value,
) -> Result<(Value, Vec<Value>), oss_query::Error> {
    let m = Mapping::from_json(mapping).unwrap();
    // The recorded statements name `compat-sql`; the planner takes the field part only.
    let sel = parse_ppl(stmt, &Limits::default())?;
    let p = plan(&sel, &m, 10_000)?;
    let body = p.body();
    let text = |f: &str| mapping["properties"][f.split('.').next().unwrap()]["type"] == "text";
    let mut hits: Vec<Value> = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        let keep = match body.get("query") {
            None => true,
            Some(q) => eval::matches(d, q, &text).unwrap_or_else(|e| panic!("{stmt}: {e}")),
        };
        if keep {
            hits.push(
                json!({"_index": "compat-sql", "_id": i.to_string(), "_score": 1.0, "_source": d}),
            );
        }
    }
    if let Some(Value::Array(keys)) = body.get("sort") {
        let mut srcs: Vec<Value> = hits.iter().map(|h| h["_source"].clone()).collect();
        let mut order: Vec<usize> = (0..srcs.len()).collect();
        eval::sort(&mut srcs, keys);
        order.sort_by_key(|i| {
            srcs.iter()
                .position(|s| *s == hits[*i]["_source"])
                .unwrap_or(0)
        });
        hits = order.into_iter().map(|i| hits[i].clone()).collect();
    }
    let answer = if let Some(aggs) = body.get("aggregations") {
        let srcs: Vec<Value> = hits.iter().map(|h| h["_source"].clone()).collect();
        json!({"hits": {"total": {"value": srcs.len()}, "hits": []}, "aggregations": eval::aggregate(&srcs, aggs)})
    } else {
        let from = body.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
        let size = body.get("size").and_then(Value::as_u64).unwrap_or(10) as usize;
        let page: Vec<Value> = hits.into_iter().skip(from).take(size).collect();
        json!({"hits": {"total": {"value": page.len(), "relation": "eq"}, "hits": page}})
    };
    let r = p.finish(rows(&p, &answer).unwrap());
    let out = write(Format::Jdbc, &p, &r, 1 << 20).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    Ok((
        v["schema"].clone(),
        v["datarows"].as_array().cloned().unwrap_or_default(),
    ))
}

#[test]
fn every_ppl_statement_gives_stock_rows_or_a_listed_refusal() {
    let data = read(dir().join("data.json"));
    let docs = data["indices"]["compat-sql"]["docs"]
        .as_array()
        .unwrap()
        .clone();
    let mapping = data["indices"]["compat-sql"]["mappings"].clone();
    let mut bad = Vec::new();
    let mut served = 0;
    for (name, stmt) in corpus() {
        if NODE.contains(&name.as_str()) {
            continue;
        }
        let later = LATER.contains(&name.as_str());
        let got = run(&stmt, &docs, &mapping);
        let c = dir().join("cases");
        let err = c.join(format!("{name}.error.json"));
        if err.exists() {
            let rec = read(err);
            match (&got, later) {
                (Err(e), false) => {
                    let (a, b) = (e.to_json(), &rec["body"]);
                    for k in ["type", "reason", "details"] {
                        // Stock's syntax error lists tokens of its own grammar; ours names the position.
                        let gap = name == "ppl-syntax-error" && k == "details";
                        if a["error"][k] != b["error"][k] && !gap {
                            bad.push(format!(
                                "{name}: error {k}: stock {} ours {}",
                                b["error"][k], a["error"][k]
                            ));
                        }
                    }
                }
                (Err(_), true) => {}
                (Ok(_), _) => bad.push(format!("{name}: stock refuses, ours answers")),
            }
            continue;
        }
        let rec = read(c.join(format!("{name}.jdbc.json")))["response"].clone();
        match (got, later) {
            (Err(_), true) => {}
            (Err(e), false) => bad.push(format!("{name}: refused ({}) {e:?}: {stmt}", e.status())),
            (Ok(_), true) => bad.push(format!("{name}: now served, remove it from LATER")),
            (Ok((schema, rows)), false) => {
                served += 1;
                let want = rec["datarows"].as_array().cloned().unwrap_or_default();
                let ordered = stmt.contains("sort") || stmt.contains("head");
                let (mut w, mut r) = (want.clone(), rows.clone());
                if !ordered {
                    w.sort_by_key(Value::to_string);
                    r.sort_by_key(Value::to_string);
                }
                if !same(&Value::Array(w), &Value::Array(r)) {
                    bad.push(format!(
                        "{name}: {stmt}\n   stock {}\n   ours  {}",
                        Value::Array(want),
                        Value::Array(rows)
                    ));
                }
                if schema != rec["schema"] {
                    bad.push(format!(
                        "{name}: schema stock {} ours {schema}",
                        rec["schema"]
                    ));
                }
            }
        }
    }
    assert!(served >= 12, "only {served} statements were served");
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}
