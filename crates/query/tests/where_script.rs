//! docs/specs/sql-scalar-functions AC-3 and AC-5: a function or arithmetic in a WHERE
//! comparison is one `script` query in the same single search, built from a fixed table with
//! every literal a parameter, and what has no form in the table is a 400.

#![allow(clippy::disallowed_methods)]

use oss_query::{Limits, Mapping, Statement, parse, plan};
use serde_json::{Value, json};

fn mapping() -> Mapping {
    Mapping::from_json(&json!({"properties": {
        "name": {"type": "keyword"}, "age": {"type": "integer"},
        "balance": {"type": "double"}, "joined": {"type": "date"},
        "bio": {"type": "text"}}}))
    .unwrap()
}

fn lower(sql: &str) -> Result<Value, oss_query::Error> {
    let Statement::Select(s) = parse(sql, &Limits::default())? else {
        panic!("not a select");
    };
    plan(&s, &mapping(), 200).map(|p| p.body().clone())
}

/// The one script of a body's filter.
fn script(body: &Value) -> &Value {
    &body["query"]["bool"]["filter"][0]["script"]["script"]
}

#[test]
fn every_function_in_the_table_is_one_script_in_the_plain_selects_search() {
    for (sql, source) in [
        (
            "abs(balance) > 10",
            "doc['balance'].size() > 0 && (Math.abs(doc['balance'].value) > params.p0)",
        ),
        (
            "upper(name) = 'A'",
            "doc['name'].size() > 0 && (doc['name'].value.toUpperCase() == params.p0)",
        ),
        (
            "lower(name) <> 'a'",
            "doc['name'].size() > 0 && (doc['name'].value.toLowerCase() != params.p0)",
        ),
        (
            "length(name) = 3",
            "doc['name'].size() > 0 && (doc['name'].value.length() == params.p0)",
        ),
        (
            "concat(name, '-', name) = 'a-a'",
            "doc['name'].size() > 0 && ((doc['name'].value + params.p0 + doc['name'].value) == params.p1)",
        ),
        (
            "age + 1 >= 3",
            "doc['age'].size() > 0 && ((doc['age'].value + params.p0) >= params.p1)",
        ),
        (
            "age - 1 < 3",
            "doc['age'].size() > 0 && ((doc['age'].value - params.p0) < params.p1)",
        ),
        (
            "age * 2 <= 3",
            "doc['age'].size() > 0 && ((doc['age'].value * params.p0) <= params.p1)",
        ),
        (
            "age / 2 > 3",
            "doc['age'].size() > 0 && ((doc['age'].value / params.p0) > params.p1)",
        ),
        (
            "age % 2 = 1",
            "doc['age'].size() > 0 && ((doc['age'].value % params.p0) == params.p1)",
        ),
        ("1 + 1 = 2", "((params.p0 + params.p1) == params.p2)"),
        ("age > 1 AND length(name) = 3", ""),
    ] {
        let body = lower(&format!("SELECT name FROM t WHERE {sql}")).expect(sql);
        let plain = lower("SELECT name FROM t").unwrap();
        assert_eq!(body["size"], plain["size"], "{sql}");
        assert_eq!(body["from"], plain["from"], "{sql}");
        assert_eq!(body["_source"], plain["_source"], "{sql}");
        if source.is_empty() {
            continue;
        }
        assert_eq!(script(&body)["source"], json!(source), "{sql}");
        assert_eq!(script(&body)["lang"], json!("painless"), "{sql}");
    }
}

#[test]
fn a_script_sits_beside_the_other_conditions_and_a_text_field_reads_its_keyword() {
    let mut m = Mapping::from_json(&json!({"properties": {
        "bio": {"type": "text", "fields": {"keyword": {"type": "keyword"}}},
        "age": {"type": "integer"}}}))
    .unwrap();
    let Statement::Select(s) = parse(
        "SELECT * FROM t WHERE age > 1 AND length(bio) = 3",
        &Limits::default(),
    )
    .unwrap() else {
        panic!("not a select");
    };
    let body = plan(&s, &m, 200).unwrap().body().clone();
    let filter = body["query"]["bool"]["filter"].as_array().unwrap();
    assert_eq!(filter.len(), 2);
    assert_eq!(filter[0], json!({"range": {"age": {"gt": 1}}}));
    assert_eq!(
        filter[1]["script"]["script"]["source"],
        json!("doc['bio.keyword'].size() > 0 && (doc['bio.keyword'].value.length() == params.p0)")
    );
    m = Mapping::default();
    assert!(plan(&s, &m, 200).is_err());
}

#[test]
fn a_plan_says_whether_it_needs_the_painless_compiler() {
    let Statement::Select(s) = parse(
        "SELECT name FROM t WHERE length(name) = 3",
        &Limits::default(),
    )
    .unwrap() else {
        panic!("not a select");
    };
    assert!(plan(&s, &mapping(), 200).unwrap().uses_script());
    let Statement::Select(s) =
        parse("SELECT name FROM t WHERE age > 3", &Limits::default()).unwrap()
    else {
        panic!("not a select");
    };
    assert!(!plan(&s, &mapping(), 200).unwrap().uses_script());
}

// AC-3: a fixed-seed sweep of literals with every character that could end a string or a comment.
#[test]
fn a_literal_never_reaches_the_source() {
    let base = lower("SELECT name FROM t WHERE upper(name) = 'a'").unwrap();
    let base_src = script(&base)["source"].clone();
    let alphabet: Vec<char> = "'\"\\*/;{}()$ \n\t%_-xyz0".chars().collect();
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    for _ in 0..500 {
        let mut lit = String::new();
        for _ in 0..(seed % 12) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            lit.push(alphabet[(seed % alphabet.len() as u64) as usize]);
        }
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let sql = format!(
            "SELECT name FROM t WHERE upper(name) = '{}'",
            lit.replace('\'', "''")
        );
        let body = lower(&sql).unwrap();
        assert_eq!(script(&body)["source"], base_src, "{lit:?}");
        assert_eq!(script(&body)["params"]["p0"], json!(lit), "{lit:?}");
    }
    let body = lower("SELECT name FROM t WHERE upper(name) = 'x'' */ + 1 //'").unwrap();
    assert_eq!(script(&body)["source"], base_src);
}

#[test]
fn what_has_no_script_form_is_a_400() {
    for sql in [
        "ifnull(name, 'a') = 'a'",
        "year(joined) = 2020",
        "date_format(joined, '%Y') = '2020'",
        "CASE WHEN age > 1 THEN 1 ELSE 0 END = 1",
        "abs(name) = 1",
        "upper(age) = 'A'",
        "concat(name, age) = 'a'",
        "upper(name) > 'A'",
        "age / 0 > 1",
        "age / balance > 1",
        "age % 0 = 1",
        "upper(joined) = 'A'",
        "length(missing) = 1",
        "length(name) = NULL",
        "reverse(name) = 'a'",
        "length(name)",
        "length(name) BETWEEN 1 AND 3",
    ] {
        let err = lower(&format!("SELECT name FROM t WHERE {sql}")).expect_err(sql);
        assert_eq!(err.status(), 400, "{sql}");
    }
}

#[test]
fn a_bad_field_name_never_reaches_the_source() {
    let m = Mapping::from_json(&json!({"properties": {"a'b": {"type": "keyword"}}})).unwrap();
    let Statement::Select(s) = parse(
        "SELECT * FROM t WHERE length(`a'b`) = 1",
        &Limits::default(),
    )
    .unwrap() else {
        panic!("not a select");
    };
    assert!(plan(&s, &m, 200).is_err());
}
