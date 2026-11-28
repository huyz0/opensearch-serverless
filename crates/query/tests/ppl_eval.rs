//! docs/specs/ppl-eval AC-2 to AC-4.

#![allow(clippy::disallowed_methods)]

use oss_query::{Format, Limits, Mapping, parse_ppl, plan, rows, write};
use serde_json::{Value, json};

fn mapping() -> Mapping {
    Mapping::from_json(&json!({"properties": {
        "name": {"type": "keyword"}, "age": {"type": "integer"}, "city": {"type": "keyword"}}}))
    .unwrap()
}

fn planned(ppl: &str) -> Result<oss_query::Plan, oss_query::Error> {
    plan(&parse_ppl(ppl, &Limits::default())?, &mapping(), 200)
}

fn answer(ppl: &str, doc: &Value) -> Value {
    let p = planned(ppl).unwrap();
    let hits = json!({"hits": {"total": {"value": 1}, "hits": [{"_source": doc}]}});
    let r = rows(&p, &hits).unwrap();
    serde_json::from_str(&write(Format::Jdbc, &p, &r, 1 << 16).unwrap()).unwrap()
}

// AC-2
#[test]
fn an_eval_reads_only_its_fields_and_adds_no_script() {
    let plain = planned("source=t | fields name").unwrap().body().clone();
    let eval = planned("source=t | eval x = age + 1 | fields name, x")
        .unwrap()
        .body()
        .clone();
    assert_eq!(eval["_source"]["includes"], json!(["name", "age"]));
    assert_eq!(eval["size"], plain["size"]);
    assert!(!eval.to_string().contains("script"));
}

// AC-3
#[test]
fn without_fields_every_field_comes_first_then_the_computed_columns() {
    let doc = json!({"name": "ann", "age": 30, "city": "x"});
    let v = answer("source=t | eval a = age * 2, b = upper(name)", &doc);
    let names: Vec<&str> = v["schema"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 5);
    assert_eq!(&names[3..], ["a", "b"]);
    let row = v["datarows"][0].as_array().unwrap();
    assert_eq!(&row[3..], [json!(60), json!("ANN")]);
    // The last expression of a repeated name wins.
    let v = answer(
        "source=t | eval a = age + 1 | eval a = age + 2 | fields a",
        &doc,
    );
    assert_eq!(v["datarows"][0][0], json!(32));
    // A computed column through a function and a condition.
    let v = answer(
        "source=t | eval big = if(age > 20, 1, 0), n = length(name) | fields big, n",
        &doc,
    );
    assert_eq!(v["datarows"][0], json!([1, 3]));
}

// AC-4
#[test]
fn a_computed_name_in_where_or_sort_is_refused() {
    for ppl in [
        "source=t | eval x = age + 1 | where x > 3",
        "source=t | eval x = age + 1 | sort x",
        "source=t | eval x = age + 1 | stats count() by x",
        "source=t | eval x = reverse(name)",
        "source=t | eval x",
        "source=t | eval = age",
    ] {
        match planned(ppl) {
            Ok(_) => panic!("{ppl} was accepted"),
            Err(e) => assert_eq!(e.status(), 400, "{ppl}"),
        }
    }
}
