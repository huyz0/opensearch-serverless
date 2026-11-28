//! docs/specs/sql-show-tables: `SHOW TABLES` and `DESCRIBE` over a pattern read the
//! descriptor listing once, keep what `like` and the guard allow, and are refused past the cap.

// ARITH: request counts are small and only grow; the fixtures are small recorded files read once.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::IndexRecord;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::sql_route::{
    DescriptorLister, SqlBackend, SqlFailure, SqlGuard, SqlLimits, SqlService, routes,
};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

/// Answers every mapping the same and refuses every search.
struct Mappings;

impl SqlBackend for Mappings {
    fn mapping(&self, _: &str) -> Result<Value, SqlFailure> {
        Ok(json!({"properties": {"age": {"type": "long"}, "city": {"type": "keyword"}}}))
    }

    fn search(&self, _: &[String], _: &Value) -> Result<Vec<u8>, SqlFailure> {
        panic!("a metadata statement runs no search");
    }
}

/// Allows every name but `secret`.
struct NoSecret;

impl SqlGuard for NoSecret {
    fn allow(&self, _: Option<&str>, _: &str, names: &[&str]) -> Result<(), String> {
        if names.contains(&"secret") {
            Err(r#"{"error":{"type":"security_exception"},"status":403}"#.to_owned())
        } else {
            Ok(())
        }
    }
}

struct Rig {
    server: Server,
    store: Arc<dyn BlobStore>,
}

fn rig(names: &[&str], listing: bool) -> Rig {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let descriptors =
        DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional).expect("names");
    for name in names {
        descriptors
            .create(
                &IndexRecord::new(name, &format!("{name}-uuid"), 1),
                1,
                &mut vec![0u8; 1 << 16],
            )
            .expect("created");
    }
    let mut service = SqlService::new(
        Arc::new(Mappings),
        Arc::new(NoSecret),
        SqlLimits {
            enabled: true,
            ppl_enabled: true,
            size_limit: 200,
            max_request_bytes: 4096,
            max_buckets: 10_000,
        },
    );
    if listing {
        let lister = DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional)
            .expect("lister names");
        service = service.with_lister(Arc::new(DescriptorLister::new(lister)));
    }
    let router = routes(Router::new(), &Arc::new(service)).expect("routes");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    Rig { server, store }
}

fn ask(r: &Rig, sql: &str) -> (u16, Value) {
    let body = json!({"query": sql}).to_string();
    let reply = support::request(
        r.server.local_addr(),
        "POST",
        "/_plugins/_sql",
        Some(body.as_bytes()),
    );
    (
        reply.status,
        serde_json::from_str(&reply.text()).unwrap_or(Value::Null),
    )
}

/// Requests of each kind sent while `f` runs.
fn sent(r: &Rig, f: impl FnOnce()) -> (u64, u64) {
    let meter = r.store.meter();
    let (lists, all) = (meter.count(S3Op::List), meter.total());
    f();
    (meter.count(S3Op::List) - lists, meter.total() - all)
}

fn tables(v: &Value) -> Vec<String> {
    v["datarows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r[2].as_str().expect("name").to_owned())
        .collect()
}

// AC-1, AC-2, AC-3
#[test]
fn show_tables_lists_the_prefix_once_and_filters_by_like() {
    let r = rig(&["logs-a", "logs-b", "logs-bb", "docs", "secret"], true);
    let mut answers = Vec::new();
    for (pattern, want) in [
        ("logs-%", vec!["logs-a", "logs-b", "logs-bb"]),
        ("logs-_", vec!["logs-a", "logs-b"]),
        ("%", vec!["docs", "logs-a", "logs-b", "logs-bb"]),
        ("%b", vec!["logs-b", "logs-bb"]),
        ("docs", vec!["docs"]),
        ("zzz%", vec![]),
    ] {
        let (counts, answer) = {
            let mut got = (0, Value::Null);
            let counts = sent(&r, || got = ask(&r, &format!("SHOW TABLES LIKE {pattern}")));
            (counts, got)
        };
        assert_eq!(answer.0, 200, "{pattern}: {}", answer.1);
        assert_eq!(tables(&answer.1), want, "{pattern}");
        // One LIST, and nothing else, whatever the pattern matches.
        assert_eq!(counts.0, 1, "{pattern}");
        oss_testkit::budget::check("sql.show_tables.requests", counts.1, "");
        answers.push(answer.1);
    }
    let first = &answers[0]["datarows"][0];
    assert_eq!(first[0], json!("serverless"));
    assert_eq!(first[3], json!("BASE TABLE"));
    assert_eq!(answers[0]["schema"][2]["name"], json!("TABLE_NAME"));
}

// AC-5
#[test]
fn a_name_the_guard_refuses_is_left_out() {
    let r = rig(&["secret", "open"], true);
    let (status, v) = ask(&r, "SHOW TABLES LIKE %");
    assert_eq!(status, 200, "{v}");
    assert_eq!(tables(&v), vec!["open"]);
    let (status, v) = ask(&r, "SHOW TABLES LIKE secret");
    assert_eq!(status, 200, "{v}");
    assert!(tables(&v).is_empty());
    let (status, v) = ask(&r, "DESCRIBE TABLES LIKE %");
    assert_eq!(status, 200, "{v}");
    assert!(
        v["datarows"]
            .as_array()
            .expect("rows")
            .iter()
            .all(|row| row[2] == json!("open"))
    );
}

// AC-1
#[test]
fn describe_over_a_pattern_answers_each_matched_index_in_name_order() {
    let r = rig(&["logs-b", "logs-a", "docs"], true);
    let (status, v) = ask(&r, "DESCRIBE TABLES LIKE logs-%");
    assert_eq!(status, 200, "{v}");
    let rows: Vec<(String, String, i64)> = v["datarows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                row[2].as_str().expect("table").to_owned(),
                row[3].as_str().expect("column").to_owned(),
                row[16].as_i64().expect("ordinal"),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("logs-a".to_owned(), "city".to_owned(), 1),
            ("logs-a".to_owned(), "age".to_owned(), 2),
            ("logs-b".to_owned(), "city".to_owned(), 1),
            ("logs-b".to_owned(), "age".to_owned(), 2),
        ]
    );
    let (_, v) = ask(&r, "DESCRIBE TABLES LIKE logs-% COLUMNS LIKE c%");
    assert_eq!(v["datarows"].as_array().expect("rows").len(), 2);
    // One named index lists nothing.
    let counts = sent(&r, || {
        let (status, _) = ask(&r, "DESCRIBE TABLES LIKE docs");
        assert_eq!(status, 200);
    });
    assert_eq!(counts, (0, 0));
}

// AC-4
#[test]
fn a_prefix_past_the_cap_is_refused_rather_than_cut() {
    let names: Vec<String> = (0..1_001).map(|n| format!("many-{n:04}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let r = rig(&refs, true);
    let (status, v) = ask(&r, "SHOW TABLES LIKE many-%");
    assert_eq!(status, 400, "{v}");
    assert!(v.to_string().contains("1000"), "{v}");
    let (status, _) = ask(&r, "DESCRIBE TABLES LIKE many-%");
    assert_eq!(status, 400);
    // A pattern that narrows to fewer is served.
    let (status, v) = ask(&r, "SHOW TABLES LIKE many-000%");
    assert_eq!(status, 200, "{v}");
    assert_eq!(tables(&v).len(), 10);
}

// AC-6
#[test]
fn a_node_that_cannot_list_refuses_and_sends_no_request() {
    let r = rig(&["a"], false);
    let counts = sent(&r, || {
        let (status, v) = ask(&r, "SHOW TABLES LIKE %");
        assert_eq!(status, 400, "{v}");
        let (status, _) = ask(&r, "DESCRIBE TABLES LIKE a%");
        assert_eq!(status, 400);
    });
    assert_eq!(counts, (0, 0));
}

fn recorded(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sql/cases")
        .join(format!("{name}.jdbc.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture"))
        .expect("fixture json");
    v["response"].clone()
}

/// The rows as strings, the cluster column dropped, in a fixed order.
fn keyed(v: &Value) -> Vec<String> {
    let mut rows: Vec<String> = v["datarows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| Value::Array(r.as_array().expect("row")[1..].to_vec()).to_string())
        .collect();
    rows.sort();
    rows
}

/// The recorded statements over the recorded documents' mapping: stock's rows, schema and all.
struct Recorded;

impl SqlBackend for Recorded {
    fn mapping(&self, _: &str) -> Result<Value, SqlFailure> {
        let data: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/sql/data.json"),
            )
            .expect("data"),
        )
        .expect("data json");
        Ok(data["indices"]["compat-sql"]["mappings"].clone())
    }

    fn search(&self, _: &[String], _: &Value) -> Result<Vec<u8>, SqlFailure> {
        panic!("a metadata statement runs no search");
    }
}

// AC-1: stock's recorded answers for the statements over two indices.
#[test]
fn the_recorded_statements_give_stocks_rows() {
    let r = rig(&["compat-sql", "compat-sql-b"], true);
    let (status, v) = ask(&r, "show tables like compat-sql%");
    let want = recorded("show-tables");
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["schema"], want["schema"]);
    assert_eq!(keyed(&v), keyed(&want));

    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let descriptors =
        DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional).expect("names");
    descriptors
        .create(
            &IndexRecord::new("compat-sql", "compat-sql-uuid", 1),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .expect("created");
    let service = SqlService::new(
        Arc::new(Recorded),
        Arc::new(NoSecret),
        SqlLimits {
            enabled: true,
            ppl_enabled: true,
            size_limit: 200,
            max_request_bytes: 4096,
            max_buckets: 10_000,
        },
    )
    .with_lister(Arc::new(DescriptorLister::new(
        DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional).expect("lister"),
    )));
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        routes(Router::new(), &Arc::new(service)).expect("routes"),
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let r = Rig { server, store };
    for name in ["describe", "describe-columns"] {
        let sql = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/sql/cases")
                .join(format!("{name}.sql")),
        )
        .expect("sql");
        let (status, v) = ask(&r, sql.trim());
        let want = recorded(name);
        assert_eq!(status, 200, "{name}: {v}");
        assert_eq!(v["schema"], want["schema"], "{name}");
        assert_eq!(keyed(&v), keyed(&want), "{name}");
    }
}
