//! docs/specs/sql-scalar-functions AC-2, AC-6, AC-7 for the select list: an expression
//! reads only its fields, adds no search, never panics, and its arithmetic is total.

#![allow(clippy::disallowed_methods)]

use oss_query::{Format, Limits, Mapping, Statement, parse, plan, rows, write};
use serde_json::{Value, json};

fn mapping() -> Mapping {
    Mapping::from_json(&json!({"properties": {
        "name": {"type": "keyword"}, "age": {"type": "integer"},
        "balance": {"type": "double"}, "city": {"type": "keyword"}}}))
    .unwrap()
}

fn planned(sql: &str) -> oss_query::Plan {
    let Statement::Select(s) = parse(sql, &Limits::default()).unwrap() else {
        panic!("not a select");
    };
    plan(&s, &mapping(), 200).unwrap()
}

// AC-2
#[test]
fn an_expression_reads_only_its_fields_in_the_same_single_search() {
    let plain = planned("SELECT age, city FROM t").body().clone();
    let expr = planned("SELECT age + 1, upper(city) FROM t").body().clone();
    assert_eq!(plain, expr);
    assert!(!expr.to_string().contains("script"));
    let one = planned("SELECT concat(name, '-', name) FROM t");
    assert_eq!(one.body()["_source"]["includes"], json!(["name"]));
}

fn cell(sql: &str, doc: &Value) -> Value {
    let p = planned(sql);
    let answer = json!({"hits": {"total": {"value": 1}, "hits": [{"_source": doc}]}});
    let r = rows(&p, &answer).unwrap();
    let out: Value = serde_json::from_str(&write(Format::Jdbc, &p, &r, 1 << 16).unwrap()).unwrap();
    out["datarows"][0][0].clone()
}

// AC-6
#[test]
fn arithmetic_is_total() {
    let d = json!({"age": 9_223_372_036_854_775_807_i64, "balance": 0.0, "name": "x"});
    assert_eq!(cell("SELECT age + 1 FROM t", &d), Value::Null);
    assert_eq!(cell("SELECT age / 0 FROM t", &d), Value::Null);
    assert_eq!(cell("SELECT age % 0 FROM t", &d), Value::Null);
    assert_eq!(cell("SELECT balance / 0 FROM t", &d), Value::Null);
    assert_eq!(cell("SELECT 7 / 2 FROM t", &d), json!(3));
    assert_eq!(cell("SELECT balance + 1.5 FROM t", &d), json!(1.5));
    assert_eq!(
        cell("SELECT abs(0 - age) FROM t", &d),
        json!(9_223_372_036_854_775_807_i64)
    );
    let none = json!({});
    assert_eq!(cell("SELECT upper(name) FROM t", &none), Value::Null);
    assert_eq!(cell("SELECT ifnull(age, -1) FROM t", &none), json!(-1));
    assert_eq!(cell("SELECT concat(name, city) FROM t", &d), Value::Null);
}

#[test]
fn unknown_functions_and_wrong_arities_are_refused() {
    for sql in [
        "SELECT reverse(name) FROM t",
        "SELECT abs() FROM t",
        "SELECT abs(age, 1) FROM t",
        "SELECT concat() FROM t",
        "SELECT nope + 1 FROM t",
        "SELECT age + 1, count(*) FROM t",
        "SELECT CASE WHEN name LIKE 'a%' THEN 1 END FROM t",
    ] {
        let parsed = parse(sql, &Limits::default());
        let refused = match parsed {
            Err(e) => e.status() == 400,
            Ok(Statement::Select(s)) => plan(&s, &mapping(), 200).is_err(),
            Ok(_) => false,
        };
        assert!(refused, "{sql}");
    }
}

// AC-7
#[test]
fn expression_depth_is_bounded_and_no_mutation_panics() {
    let deep = format!("SELECT {}age{} FROM t", "(".repeat(100), ")".repeat(100));
    assert!(parse(&deep, &Limits::default()).is_err());
    let chain = format!("SELECT {} FROM t", vec!["age"; 5000].join(" + "));
    assert!(parse(&chain, &Limits::default()).is_err());
    let seeds = [
        "SELECT age + 1, upper(name), concat(name, '-', city) FROM t",
        "SELECT CASE WHEN age > 30 THEN 'old' WHEN age IS NULL THEN 'n' ELSE 'young' END FROM t",
        "SELECT if(age > 30 AND NOT city = 'x', 1, 2), date_format(name, '%Y-%m') FROM t",
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = b"()'`\" ,.*=<>!%_-/+\\\n0123456789abcXYZ";
    for seed in seeds {
        for _ in 0..4000 {
            let mut bytes = seed.as_bytes().to_vec();
            for _ in 0..(next() % 6) {
                let len = u64::try_from(bytes.len()).unwrap().max(1);
                let at = usize::try_from(next() % len).unwrap();
                let k = usize::try_from(next() % u64::try_from(alphabet.len()).unwrap()).unwrap();
                match next() % 3 {
                    0 => bytes.truncate(at),
                    1 if at < bytes.len() => bytes[at] = alphabet[k],
                    _ => bytes.insert(at.min(bytes.len()), alphabet[k]),
                }
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Ok(Statement::Select(s)) = parse(&text, &Limits::default())
                && let Ok(p) = plan(&s, &mapping(), 200)
            {
                let answer = json!({"hits": {"hits": [{"_source": {"age": 3, "name": "a"}}]}});
                let _ = rows(&p, &answer);
            }
        }
    }
}
