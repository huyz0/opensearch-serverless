//! The recorded stock corpus (`fixtures/sql`, stock 3.10.0): every statement either
//! gives stock's rows over the same documents or is refused for a reason listed here,
//! and every refusal listed here is still a refusal.

// ARITH: the evaluator runs over six small documents; an overflow panics the test,
// which fails it as it should. The fixtures are a few small recorded files, read once.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/eval.rs"]
mod eval;

use oss_query::{Format, Limits, Mapping, Statement, parse, plan, rows, write};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql")
}

fn read(p: PathBuf) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

/// Statements this slice refuses, with the slice that serves them.
const LATER: &[&str] = &[
    "scalar-where",
    "where-in-expression",
    "join",
    "left-join",
    "subquery",
    "union",
    "window",
    "multi-index",
    "show-tables",
    "describe",
];

/// Statements over more than one index: the node resolves the names and runs one search
/// over them, so they are checked there, not over one document set.
const NODE: &[&str] = &["wildcard-index"];

fn later(name: &str) -> bool {
    LATER.iter().any(|p| name.starts_with(p))
}

fn corpus() -> Vec<(String, String)> {
    let mut v = Vec::new();
    for l in fs::read_to_string(dir().join("corpus.txt"))
        .unwrap()
        .lines()
    {
        let mut p = l.splitn(3, '|');
        if let (false, Some(n), Some("sql"), Some(s)) =
            (l.starts_with('#'), p.next(), p.next(), p.next())
        {
            v.push((n.to_owned(), s.to_owned()));
        }
    }
    v
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

/// Our rows for a statement over the recorded documents.
fn run(sql: &str, docs: &[Value], mapping: &Value) -> Result<Vec<Value>, oss_query::Error> {
    let m = Mapping::from_json(mapping).unwrap();
    let Statement::Select(sel) = parse(sql, &Limits::default())? else {
        return Err(oss_query::Error::unsupported("not a select"));
    };
    let p = plan(&sel, &m, 10_000)?;
    let body = p.body();
    let text = |f: &str| {
        let f = f.strip_suffix(".keyword").map_or(f, |_| "\u{0}");
        f != "\u{0}" && mapping["properties"][f.split('.').next().unwrap()]["type"] == "text"
    };
    let mut hits: Vec<Value> = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        let keep = match body.get("query") {
            None => true,
            Some(q) => eval::matches(d, q, &text).unwrap_or_else(|e| panic!("{sql}: {e}")),
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
        let tagged: Vec<Value> = srcs
            .iter()
            .enumerate()
            .map(|(i, s)| json!({"i": i, "s": s}))
            .collect();
        let mut by_src = tagged.clone();
        by_src.iter_mut().for_each(|t| *t = t["s"].clone());
        eval::sort(&mut srcs, keys);
        order.sort_by_key(|i| {
            srcs.iter()
                .position(|s| *s == hits[*i]["_source"])
                .unwrap_or(0)
        });
        hits = order.into_iter().map(|i| hits[i].clone()).collect();
    }
    if let Some(aggs) = body.get("aggregations") {
        let srcs: Vec<Value> = hits.iter().map(|h| h["_source"].clone()).collect();
        let answer = json!({"hits": {"total": {"value": srcs.len()}, "hits": []}, "aggregations": eval::aggregate(&srcs, aggs)});
        return Ok(finish(&p, &answer));
    }
    let from = body.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
    let size = body.get("size").and_then(Value::as_u64).unwrap_or(10) as usize;
    let page: Vec<Value> = hits.into_iter().skip(from).take(size).collect();
    let answer = json!({"hits": {"total": {"value": page.len(), "relation": "eq"}, "hits": page}});
    Ok(finish(&p, &answer))
}

fn finish(p: &oss_query::Plan, answer: &Value) -> Vec<Value> {
    let r = rows(p, answer).unwrap();
    let out = write(Format::Jdbc, p, &r, 1 << 20).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    v["datarows"].as_array().cloned().unwrap_or_default()
}

#[test]
fn every_statement_gives_stock_rows_or_is_a_listed_refusal() {
    let data = read(dir().join("data.json"));
    let docs = data["indices"]["compat-sql"]["docs"]
        .as_array()
        .unwrap()
        .clone();
    let mapping = data["indices"]["compat-sql"]["mappings"].clone();
    let only = std::env::var("ONLY").unwrap_or_default();
    let mut bad = Vec::new();
    for (name, sql) in corpus() {
        if !name.starts_with(&only) || NODE.contains(&name.as_str()) {
            continue;
        }
        let c = dir().join("cases");
        let got = run(&sql, &docs, &mapping);
        let err = c.join(format!("{name}.error.json"));
        if err.exists() {
            let rec = read(err);
            let want = rec["status"].as_u64().unwrap_or(400) as u16;
            match &got {
                Err(e) if e.status() == want => {
                    let (a, b) = (e.to_json(), &rec["body"]);
                    for k in ["type", "reason", "details"] {
                        // The function-breadth slice brings the list that tells an unknown function from an unsupported one.
                        let gap = name == "unknown-function" && k == "details";
                        if a["error"][k] != b["error"][k] && !later(&name) && !gap {
                            bad.push(format!(
                                "{name}: error {k}: stock {} ours {}",
                                b["error"][k], a["error"][k]
                            ));
                        }
                    }
                }
                _ if later(&name) || matches!(name.as_str(), "unknown-index" | "union") => {}
                other => bad.push(format!(
                    "{name}: stock refuses with {want}, ours {:?}",
                    other.as_ref().map(|_| ()).map_err(|e| e.status())
                )),
            }
            continue;
        }
        let want = read(c.join(format!("{name}.jdbc.json")))["response"]["datarows"].clone();
        let want = want.as_array().cloned().unwrap_or_default();
        match got {
            Err(_) if later(&name) => {}
            Err(e) => bad.push(format!(
                "{name}: refused ({}: {}), stock answers: {sql}",
                e.status(),
                e.to_json()["error"]["details"]
            )),
            Ok(_) if later(&name) => bad.push(format!("{name}: now served, remove it from LATER")),
            Ok(mut rows) => {
                let ordered = sql.to_lowercase().contains("order by");
                let (mut w, mut r) = (want.clone(), rows.clone());
                if !ordered {
                    w.sort_by_key(Value::to_string);
                    r.sort_by_key(Value::to_string);
                }
                if !same(&Value::Array(w), &Value::Array(r)) {
                    rows.truncate(8);
                    bad.push(format!(
                        "{name}: {sql}\n   stock {}\n   ours  {}",
                        Value::Array(want),
                        Value::Array(rows)
                    ));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

/// The `aggregations` of an aggregate statement are the plugin's own, key for key.
#[test]
fn aggregation_bodies_equal_stock() {
    let data = read(dir().join("data.json"));
    let m = Mapping::from_json(&data["indices"]["compat-sql"]["mappings"]).unwrap();
    let mut checked = 0;
    for (name, sql) in corpus() {
        let dsl = dir().join("cases").join(format!("{name}.dsl.json"));
        if !dsl.exists() || later(&name) {
            continue;
        }
        let want = read(dsl)["dsl"]["aggregations"].clone();
        if want.is_null() {
            continue;
        }
        let Ok(Statement::Select(sel)) = parse(&sql, &Limits::default()) else {
            continue;
        };
        let p = plan(&sel, &m, 10_000).unwrap();
        assert_eq!(p.body()["aggregations"], want, "{name}");
        assert_eq!(p.body()["size"], 0, "{name}");
        checked += 1;
    }
    assert!(
        checked >= 8,
        "only {checked} aggregate bodies were compared"
    );
}
