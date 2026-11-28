//! docs/specs/sql-scalar-functions AC-2, AC-6, AC-7 for the select list: an expression
//! reads only its fields, adds no search, never panics, and its arithmetic is total.

#![allow(clippy::disallowed_methods)]

use oss_query::{Format, Limits, Mapping, Statement, parse, plan, rows, write};
use serde_json::{Value, json};

fn mapping() -> Mapping {
    Mapping::from_json(&json!({"properties": {
        "name": {"type": "keyword"}, "age": {"type": "integer"},
        "balance": {"type": "double"}, "city": {"type": "keyword"},
        "joined": {"type": "date"}, "none": {"type": "keyword"}, "absent": {"type": "long"}}}))
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

fn doc() -> Value {
    json!({"name": "Ann", "age": 30, "balance": 10.5, "city": "x",
        "joined": "2024-03-05T07:08:09Z", "none": null})
}

// AC-1, per function and per operator: each value is the one stock's text gives.
#[test]
fn every_function_and_date_specifier_gives_its_value() {
    let d = doc();
    for (sql, want) in [
        ("SELECT lower(name) FROM t", json!("ann")),
        ("SELECT upper(name) FROM t", json!("ANN")),
        ("SELECT length(name) FROM t", json!(3)),
        ("SELECT year(joined) FROM t", json!(2024)),
        ("SELECT month(joined) FROM t", json!(3)),
        ("SELECT date_format(joined, '%Y') FROM t", json!("2024")),
        ("SELECT date_format(joined, '%m') FROM t", json!("03")),
        ("SELECT date_format(joined, '%d') FROM t", json!("05")),
        ("SELECT date_format(joined, '%H') FROM t", json!("07")),
        ("SELECT date_format(joined, '%i') FROM t", json!("08")),
        ("SELECT date_format(joined, '%s') FROM t", json!("09")),
        (
            "SELECT date_format(joined, '100%% %Y/%m') FROM t",
            json!("100% 2024/03"),
        ),
        ("SELECT date_format(joined, '%Q') FROM t", Value::Null),
        ("SELECT date_format(name, '%Y') FROM t", Value::Null),
        ("SELECT concat(name, '-', age) FROM t", json!("Ann-30")),
        ("SELECT ifnull(none, 'd') FROM t", json!("d")),
        ("SELECT ifnull(name, 'd') FROM t", json!("Ann")),
        ("SELECT abs(-2.5) FROM t", json!(2.5)),
        ("SELECT abs(age) FROM t", json!(30)),
        ("SELECT age + 2 FROM t", json!(32)),
        ("SELECT age - 2 FROM t", json!(28)),
        ("SELECT age * 2 FROM t", json!(60)),
        ("SELECT age / 4 FROM t", json!(7)),
        ("SELECT age % 7 FROM t", json!(2)),
        ("SELECT balance + 1 FROM t", json!(11.5)),
        ("SELECT balance - 0.5 FROM t", json!(10.0)),
        ("SELECT balance * 2 FROM t", json!(21.0)),
        ("SELECT balance / 2 FROM t", json!(5.25)),
        ("SELECT balance % 4 FROM t", json!(2.5)),
        ("SELECT 2 + 3 * 4 FROM t", json!(14)),
        ("SELECT (2 + 3) * 4 FROM t", json!(20)),
        ("SELECT 10 - 4 - 3 FROM t", json!(3)),
    ] {
        assert_eq!(cell(sql, &d), want, "{sql}");
    }
}

#[test]
fn case_conditions_follow_their_operators() {
    let d = doc();
    for (cond, want) in [
        ("age = 30", true),
        ("age <> 30", false),
        ("age < 31", true),
        ("age <= 30", true),
        ("age > 30", false),
        ("age >= 30", true),
        ("name = 'Ann'", true),
        ("name < 'Bob'", true),
        ("none IS NULL", true),
        ("name IS NULL", false),
        ("name IS NOT NULL", true),
        ("age > 20 AND name = 'Ann'", true),
        ("age > 40 OR name = 'Ann'", true),
        ("age > 40 OR name = 'Bob'", false),
        ("NOT age > 40", true),
        ("none > 1", false),
        ("absent = 1", false),
    ] {
        let sql = format!("SELECT CASE WHEN {cond} THEN 1 ELSE 0 END FROM t");
        assert_eq!(cell(&sql, &d), json!(i32::from(want)), "{cond}");
    }
    assert_eq!(
        cell("SELECT CASE WHEN age > 99 THEN 1 END FROM t", &d),
        Value::Null
    );
    assert_eq!(
        cell(
            "SELECT CASE WHEN age > 99 THEN 1 WHEN age > 9 THEN 2 ELSE 3 END FROM t",
            &d
        ),
        json!(2)
    );
    assert_eq!(cell("SELECT if(age > 20, 'a', 'b') FROM t", &d), json!("a"));
}

#[test]
fn column_types_and_names_follow_the_recordings() {
    let ty = |sql: &str| {
        let p = planned(sql);
        let out: Value =
            serde_json::from_str(&write(Format::Jdbc, &p, &[vec![Value::Null]], 1 << 16).unwrap())
                .unwrap();
        (
            out["schema"][0]["name"].clone(),
            out["schema"][0]["type"].clone(),
        )
    };
    assert_eq!(
        ty("SELECT age + 1 FROM t"),
        (json!("age + 1"), json!("integer"))
    );
    assert_eq!(ty("SELECT balance * 2 FROM t").1, json!("double"));
    assert_eq!(ty("SELECT abs(-3) FROM t").1, json!("integer"));
    assert_eq!(ty("SELECT abs(balance) FROM t").1, json!("double"));
    assert_eq!(ty("SELECT length(name) FROM t").1, json!("integer"));
    assert_eq!(ty("SELECT year(name) FROM t").1, json!("integer"));
    assert_eq!(ty("SELECT upper(name) AS u FROM t").0, json!("upper(name)"));
    assert_eq!(ty("SELECT 1.5 FROM t").1, json!("double"));
    assert_eq!(ty("SELECT 'a' FROM t").1, json!("keyword"));
}
