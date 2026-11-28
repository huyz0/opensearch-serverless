//! AC-5, AC-9, AC-10: result rows and the three text formats (provisional:
//! hand-written from the plugin's documented output).

mod common;

use common::mapping;
use oss_query::{Format, Limits, Statement, parse, plan, rows, write};
use serde_json::json;

fn planned(sql: &str) -> oss_query::Plan {
    let Statement::Select(sel) = parse(sql, &Limits::default()).unwrap() else {
        panic!()
    };
    plan(&sel, &mapping(), 200).unwrap()
}

fn response() -> serde_json::Value {
    json!({"hits": {"total": {"value": 2, "relation": "eq"}, "hits": [
        {"_index": "accounts", "_id": "1", "_score": 1.5,
         "_source": {"firstname": "Amber", "age": 32, "city": "Brogan", "employer": {"name": "Pyrami"}}},
        {"_index": "accounts", "_id": "2", "_score": 0.5,
         "_source": {"firstname": "Hattie, Jr", "age": null, "employer": {"name": "=SUM(1)"}}}
    ]}})
}

#[test]
fn jdbc_has_schema_datarows_total_size_and_status() {
    let p =
        planned("SELECT firstname AS name, age, city, employer.name, _id, _score FROM accounts");
    let r = rows(&p, &response()).unwrap();
    let out: serde_json::Value =
        serde_json::from_str(&write(Format::Jdbc, &p, &r, 1 << 20).unwrap()).unwrap();
    assert_eq!(
        out["schema"],
        json!([
            {"name": "firstname", "alias": "name", "type": "text"},
            {"name": "age", "type": "long"},
            {"name": "city", "type": "keyword"},
            {"name": "employer.name", "type": "keyword"},
            {"name": "_id", "type": "keyword"},
            {"name": "_score", "type": "float"}
        ])
    );
    assert_eq!(
        out["datarows"],
        json!([
            ["Amber", 32, "Brogan", "Pyrami", "1", 1.5],
            ["Hattie, Jr", null, null, "=SUM(1)", "2", 0.5]
        ])
    );
    assert_eq!(out["total"], 2);
    assert_eq!(out["size"], 2);
    assert_eq!(out["status"], 200);
}

#[test]
fn csv_quotes_and_neutralises_formulas_and_raw_is_pipe_separated() {
    let p = planned("SELECT firstname, age, employer.name FROM accounts");
    let r = rows(&p, &response()).unwrap();
    let csv = write(Format::Csv, &p, &r, 1 << 20).unwrap();
    assert_eq!(
        csv,
        "firstname,age,employer.name\nAmber,32,Pyrami\n\"Hattie, Jr\",,\"'=SUM(1)\"\n"
    );
    let raw = write(Format::Raw, &p, &r, 1 << 20).unwrap();
    assert_eq!(raw, "Amber|32|Pyrami\nHattie, Jr||=SUM(1)\n");
}

#[test]
fn a_result_past_the_byte_bound_is_refused_not_truncated() {
    let p = planned("SELECT firstname FROM accounts");
    let r = rows(&p, &response()).unwrap();
    let err = write(Format::Jdbc, &p, &r, 16).unwrap_err();
    assert_eq!(err.status(), 429);
}

#[test]
fn select_star_lists_top_level_fields_in_the_plugins_hash_order() {
    let p = planned("SELECT * FROM accounts");
    let names: Vec<&str> = p.columns().iter().map(|c| c.name()).collect();
    // An object is one column; the order is a Java HashMap's over the names, as recorded
    // for the stock plugin in fixtures/sql (all-fields).
    assert!(names.contains(&"employer") && !names.contains(&"employer.name"));
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            "active",
            "address",
            "age",
            "balance",
            "city",
            "created",
            "employer",
            "firstname",
            "ip",
            "lastname"
        ]
    );
}

#[test]
fn show_and_describe_have_the_documented_columns() {
    let out: serde_json::Value = serde_json::from_str(
        &oss_query::show_tables("docker-cluster", &["accounts".into(), "logs-1".into()], 200)
            .unwrap(),
    )
    .unwrap();
    let cols: Vec<&str> = out["schema"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        cols,
        [
            "TABLE_CAT",
            "TABLE_SCHEM",
            "TABLE_NAME",
            "TABLE_TYPE",
            "REMARKS",
            "TYPE_CAT",
            "TYPE_SCHEM",
            "TYPE_NAME",
            "SELF_REFERENCING_COL_NAME",
            "REF_GENERATION"
        ]
    );
    assert_eq!(out["datarows"][1][2], "logs-1");
    let d: serde_json::Value = serde_json::from_str(
        &oss_query::describe("docker-cluster", "accounts", &mapping(), None, 200).unwrap(),
    )
    .unwrap();
    assert_eq!(d["schema"].as_array().unwrap().len(), 24);
    assert_eq!(d["datarows"][0][3], "active");
    assert_eq!(d["datarows"].as_array().unwrap().len(), 10);
}

#[test]
fn show_and_describe_parse_with_their_patterns() {
    assert!(
        matches!(parse("SHOW TABLES LIKE 'logs-%'", &Limits::default()).unwrap(), Statement::ShowTables { pattern } if pattern == "logs-%")
    );
    assert!(
        matches!(parse("DESCRIBE TABLES LIKE accounts COLUMNS LIKE 'a%'", &Limits::default()).unwrap(), Statement::DescribeTables { pattern, columns } if pattern == "accounts" && columns.as_deref() == Some("a%"))
    );
}
