//! docs/specs/sql-select slice 1b: the SQL routes over a counting backend
//! (AC-1, AC-4, AC-6, AC-11 and the routes themselves). The backend counts
//! its calls, so "no search ran" is a number.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_mem::MemoryBudget;
use oss_node::sql_route::{SqlBackend, SqlFailure, SqlGuard, SqlLimits, SqlService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

#[derive(Default)]
struct Counting {
    mappings: AtomicUsize,
    searches: AtomicUsize,
    last_body: std::sync::Mutex<Option<Value>>,
}

impl SqlBackend for Counting {
    fn mapping(&self, _: &str) -> Result<Value, SqlFailure> {
        self.mappings.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"properties": {
            "firstname": {"type": "text", "fields": {"keyword": {"type": "keyword"}}},
            "age": {"type": "long"},
            "city": {"type": "keyword"}
        }}))
    }

    fn search(&self, _: &[String], body: &Value) -> Result<Vec<u8>, SqlFailure> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        *self.last_body.lock().unwrap() = Some(body.clone());
        if body["aggregations"].get("count(*)").is_some() {
            return Ok(json!({"aggregations": {"count(*)": {"value": 6.0}}})
                .to_string()
                .into_bytes());
        }
        if let Some(comp) = body["aggregations"].get("composite_buckets") {
            // Two pages of groups: a, b and then c.
            let bucket = |k: &str, n: u64| json!({"key": {"city": k}, "doc_count": n, "count(*)": {"value": n}});
            let page = if comp["composite"].get("after").is_none() {
                json!({"buckets": [bucket("a", 3), bucket("b", 1)], "after_key": {"city": "b"}})
            } else {
                json!({"buckets": [bucket("c", 2)]})
            };
            return Ok(
                json!({"hits": {"total": {"value": 0, "relation": "eq"}, "hits": []},
                "aggregations": {"composite_buckets": page}})
                .to_string()
                .into_bytes(),
            );
        }
        Ok(
            json!({"hits": {"total": {"value": 1, "relation": "eq"}, "hits": [
                {"_index": "accounts", "_id": "1", "_score": 1.0,
                 "_source": {"firstname": "Amber", "age": 32, "city": "Brogan"}}
            ]}})
            .to_string()
            .into_bytes(),
        )
    }
}

/// Allows every name but `secret`.
struct NoSecret;

impl SqlGuard for NoSecret {
    fn allow(&self, _: Option<&str>, _: &str, names: &[&str]) -> Result<(), String> {
        if names.contains(&"secret") {
            Err(
                r#"{"error":{"type":"security_exception","reason":"no permissions"},"status":403}"#
                    .to_owned(),
            )
        } else {
            Ok(())
        }
    }
}

struct Rig {
    server: Server,
    backend: Arc<Counting>,
}

fn rig(enabled: bool) -> Rig {
    rig_with(enabled, 10_000)
}

fn rig_with(enabled: bool, max_buckets: usize) -> Rig {
    let backend = Arc::new(Counting::default());
    let service = Arc::new(SqlService::new(
        Arc::clone(&backend) as _,
        Arc::new(NoSecret),
        SqlLimits {
            enabled,
            ppl_enabled: true,
            size_limit: 200,
            max_request_bytes: 4096,
            max_buckets,
        },
    ));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, backend }
}

fn post(r: &Rig, target: &str, sql: &str) -> support::Reply {
    let body = json!({"query": sql}).to_string();
    support::request(r.server.local_addr(), "POST", target, Some(body.as_bytes()))
}

fn json_of(r: &support::Reply) -> Value {
    serde_json::from_str(&r.text()).unwrap_or_else(|e| panic!("{e}: {}", r.text()))
}

// AC-1
#[test]
fn a_node_with_the_setting_off_answers_the_disabled_error_and_runs_nothing() {
    let r = rig(false);
    let reply = post(&r, "/_plugins/_sql", "SELECT * FROM accounts");
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert_eq!(
        json_of(&reply)["error"]["type"],
        "SQLFeatureDisabledException"
    );
    assert_eq!(r.backend.mappings.load(Ordering::SeqCst), 0);
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
}

#[test]
fn a_select_runs_one_search_with_the_lowered_body_and_answers_jdbc() {
    let r = rig(true);
    let reply = post(
        &r,
        "/_plugins/_sql",
        "SELECT firstname, age FROM accounts WHERE city = 'Brogan' ORDER BY age DESC LIMIT 5",
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let out = json_of(&reply);
    assert_eq!(out["datarows"], json!([["Amber", 32]]));
    assert_eq!(
        out["schema"][0],
        json!({"name": "firstname", "type": "text"})
    );
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 1);
    assert_eq!(r.backend.mappings.load(Ordering::SeqCst), 1);
    let body = r.backend.last_body.lock().unwrap().clone().unwrap();
    assert_eq!(body["size"], 5);
    assert_eq!(
        body["sort"][0],
        json!({"age": {"order": "desc", "missing": "_last"}})
    );
    assert_eq!(
        body["query"],
        json!({"bool": {"filter": [{"term": {"city": "Brogan"}}]}})
    );
}

#[test]
fn both_prefixes_and_both_methods_are_served() {
    let r = rig(true);
    for path in ["/_plugins/_sql", "/_opendistro/_sql"] {
        let reply = post(&r, path, "SELECT age FROM accounts");
        assert_eq!(reply.status, 200, "{path}: {}", reply.text());
    }
    let body = json!({"query": "SELECT age FROM accounts"}).to_string();
    let get = support::request(
        r.server.local_addr(),
        "GET",
        "/_plugins/_sql",
        Some(body.as_bytes()),
    );
    assert_eq!(get.status, 200, "{}", get.text());
}

#[test]
fn csv_and_raw_formats_and_an_unsupported_one() {
    let r = rig(true);
    let csv = post(
        &r,
        "/_plugins/_sql?format=csv",
        "SELECT firstname, age FROM accounts",
    );
    assert_eq!(
        (csv.status, csv.text().as_str()),
        (200, "firstname,age\nAmber,32\n")
    );
    let raw = post(
        &r,
        "/_plugins/_sql?format=raw",
        "SELECT firstname, age FROM accounts",
    );
    assert_eq!(raw.text(), "Amber|32\n");
    let other = post(&r, "/_plugins/_sql?format=json", "SELECT age FROM accounts");
    assert_eq!(other.status, 400, "{}", other.text());
}

// AC-4
#[test]
fn what_cannot_be_pushed_is_a_400_and_no_search_runs() {
    let r = rig(true);
    for sql in [
        "SELECT city, count(distinct age) FROM accounts GROUP BY city",
        "SELECT reverse(city) FROM accounts",
        "SELECT * FROM accounts WHERE age + 1 > 3",
        "DELETE FROM accounts",
    ] {
        let reply = post(&r, "/_plugins/_sql", sql);
        assert_eq!(reply.status, 400, "{sql}: {}", reply.text());
    }
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
}

#[test]
fn a_syntax_error_has_the_plugins_shape_and_costs_nothing() {
    let r = rig(true);
    let reply = post(&r, "/_plugins/_sql", "select from");
    assert_eq!(reply.status, 400);
    assert_eq!(json_of(&reply)["error"]["type"], "ParserException");
    let reply = post(&r, "/_plugins/_sql", "SELEKT *");
    assert_eq!(
        json_of(&reply)["error"]["type"],
        "SQLFeatureNotSupportedException"
    );
    assert_eq!(r.backend.mappings.load(Ordering::SeqCst), 0);
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
}

// AC-6
#[test]
fn a_name_the_caller_may_not_read_is_a_403_before_any_mapping_or_search() {
    let r = rig(true);
    let reply = post(&r, "/_plugins/_sql", "SELECT * FROM secret");
    assert_eq!(reply.status, 403, "{}", reply.text());
    assert_eq!(json_of(&reply)["error"]["type"], "security_exception");
    let describe = post(&r, "/_plugins/_sql", "DESCRIBE TABLES LIKE secret");
    assert_eq!(describe.status, 403, "{}", describe.text());
    assert_eq!(r.backend.mappings.load(Ordering::SeqCst), 0);
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
}

#[test]
fn explain_plans_without_searching() {
    let r = rig(true);
    let reply = post(
        &r,
        "/_plugins/_sql/_explain",
        "SELECT age FROM accounts WHERE age > 3",
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let out = json_of(&reply);
    assert_eq!(
        out["root"]["description"]["request"]["sourceBuilder"]["size"],
        200
    );
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
    assert_eq!(r.backend.mappings.load(Ordering::SeqCst), 1);
}

#[test]
fn describe_one_index_answers_its_columns() {
    let r = rig(true);
    let reply = post(&r, "/_plugins/_sql", "DESCRIBE TABLES LIKE accounts");
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(json_of(&reply)["datarows"].as_array().unwrap().len(), 3);
}

// AC-11
#[test]
fn a_body_past_the_limit_is_a_413_and_runs_nothing() {
    let r = rig(true);
    let big = format!(
        "{{\"query\":\"SELECT * FROM accounts WHERE city = '{}'\"}}",
        "x".repeat(5000)
    );
    let reply = support::request(
        r.server.local_addr(),
        "POST",
        "/_plugins/_sql",
        Some(big.as_bytes()),
    );
    assert_eq!(reply.status, 413, "{}", reply.text());
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
    let bad = support::request(
        r.server.local_addr(),
        "POST",
        "/_plugins/_sql",
        Some(b"{not json"),
    );
    assert_eq!(bad.status, 400, "{}", bad.text());
}

// AC-5 (aggregations): a grouped statement is one search a page of the composite.
#[test]
fn a_grouped_statement_pages_the_composite_until_it_is_exhausted() {
    let r = rig(true);
    let reply = post(
        &r,
        "/_plugins/_sql",
        "SELECT city, count(*) FROM accounts GROUP BY city",
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let out = json_of(&reply);
    assert_eq!(out["datarows"], json!([["a", 3], ["b", 1], ["c", 2]]));
    assert_eq!(out["total"], 3);
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 2);
}

// AC-5 (aggregations): past the bucket bound the statement stops asking.
#[test]
fn past_the_bucket_bound_the_statement_is_refused_and_no_more_pages_are_asked() {
    let r = rig_with(true, 2);
    let reply = post(
        &r,
        "/_plugins/_sql",
        "SELECT city, count(*) FROM accounts GROUP BY city",
    );
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("more than 2 groups"));
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 2);
}

// AC-2 (aggregations): an ungrouped aggregate is one search.
#[test]
fn an_ungrouped_aggregate_is_one_search() {
    let r = rig(true);
    let reply = post(&r, "/_plugins/_sql", "SELECT count(*) FROM accounts");
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 1);
}

#[test]
fn a_select_with_no_from_answers_one_row_and_makes_no_call() {
    let r = rig(true);
    let reply = post(&r, "/_plugins/_sql", "SELECT abs(-3), 1 + 1");
    assert_eq!(reply.status, 200, "{}", reply.text());
    let out = json_of(&reply);
    assert_eq!(out["datarows"], json!([[3, 2]]));
    assert_eq!(
        out["schema"][0],
        json!({"name": "abs(-3)", "type": "integer"})
    );
    let explained = post(&r, "/_plugins/_sql/_explain", "SELECT 1 + 1");
    assert_eq!(explained.status, 200, "{}", explained.text());
    let refused = post(&r, "/_plugins/_sql", "SELECT abs(-3) WHERE 1 = 1");
    assert_eq!(refused.status, 400, "{}", refused.text());
    assert_eq!(r.backend.mappings.load(Ordering::SeqCst), 0);
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 0);
}
