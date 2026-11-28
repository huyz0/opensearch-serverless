//! docs/specs/ppl-select: the PPL routes over a counting backend (the setting, both
//! prefixes, one search a statement, the PPL answer shape, permission before search).

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
    searches: AtomicUsize,
}

impl SqlBackend for Counting {
    fn mapping(&self, _: &str) -> Result<Value, SqlFailure> {
        Ok(
            json!({"properties": {"name": {"type": "keyword"}, "age": {"type": "long"},
            "city": {"type": "keyword"}}}),
        )
    }

    fn search(&self, _: &[String], body: &Value) -> Result<Vec<u8>, SqlFailure> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        if let Some(comp) = body["aggregations"].get("composite_buckets") {
            let _ = comp;
            let b = |k: &str, n: u64| json!({"key": {"city": k}, "doc_count": n, "count(*)": {"value": n}});
            return Ok(
                json!({"hits": {"total": {"value": 0, "relation": "eq"}, "hits": []},
                "aggregations": {"composite_buckets": {"buckets": [b("a", 1), b("b", 3)]}}})
                .to_string()
                .into_bytes(),
            );
        }
        Ok(
            json!({"hits": {"total": {"value": 1, "relation": "eq"}, "hits": [
            {"_index": "accounts", "_id": "1", "_score": 1.0,
             "_source": {"name": "Amber", "age": 32, "city": "Brogan"}}]}})
            .to_string()
            .into_bytes(),
        )
    }
}

struct NoSecret;

impl SqlGuard for NoSecret {
    fn allow(&self, _: Option<&oss_rest::Identity>, _: &str, names: &[&str]) -> Result<(), String> {
        if names.contains(&"secret") {
            Err(r#"{"error":{"type":"security_exception"},"status":403}"#.to_owned())
        } else {
            Ok(())
        }
    }
}

fn rig(ppl_enabled: bool) -> (Server, Arc<Counting>) {
    let backend = Arc::new(Counting::default());
    let service = Arc::new(SqlService::new(
        Arc::clone(&backend) as _,
        Arc::new(NoSecret),
        SqlLimits {
            enabled: true,
            ppl_enabled,
            size_limit: 200,
            max_request_bytes: 4096,
            max_buckets: 10_000,
        },
    ));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    (server, backend)
}

fn post(s: &Server, target: &str, q: &str) -> (u16, Value) {
    let body = json!({"query": q}).to_string();
    let r = support::request(s.local_addr(), "POST", target, Some(body.as_bytes()));
    let v = serde_json::from_str(&r.text()).unwrap_or(Value::Null);
    (r.status, v)
}

#[test]
fn with_the_setting_off_ppl_answers_the_disabled_error_and_runs_nothing() {
    let (s, b) = rig(false);
    let (status, v) = post(&s, "/_plugins/_ppl", "source=accounts");
    assert_eq!(status, 400);
    assert_eq!(v["error"]["type"], "SQLFeatureDisabledException");
    assert_eq!(v["error"]["reason"], "Invalid PPL query");
    assert_eq!(b.searches.load(Ordering::SeqCst), 0);
}

#[test]
fn a_pipeline_is_one_search_and_answers_without_a_status() {
    let (s, b) = rig(true);
    for path in ["/_plugins/_ppl", "/_opendistro/_ppl"] {
        let (status, v) = post(
            &s,
            path,
            "source=accounts | where age > 20 | fields name, age",
        );
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["datarows"], json!([["Amber", 32]]));
        assert!(v.get("status").is_none(), "{v}");
        assert_eq!(v["schema"][1], json!({"name": "age", "type": "bigint"}));
    }
    assert_eq!(b.searches.load(Ordering::SeqCst), 2);
}

#[test]
fn stats_rows_are_ordered_after_the_pages_are_read() {
    let (s, _) = rig(true);
    let (status, v) = post(
        &s,
        "/_plugins/_ppl",
        "source=accounts | stats count() as c by city | sort - c",
    );
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["datarows"], json!([[3, "b"], [1, "a"]]));
}

#[test]
fn what_cannot_be_pushed_and_syntax_errors_cost_no_search() {
    let (s, b) = rig(true);
    let (status, _) = post(&s, "/_plugins/_ppl", "source=accounts | eval x = age * 2");
    assert_eq!(status, 400);
    let (status, v) = post(&s, "/_plugins/_ppl", "source=accounts | where");
    assert_eq!(status, 400);
    assert_eq!(v["error"]["type"], "SyntaxCheckException");
    assert_eq!(b.searches.load(Ordering::SeqCst), 0);
}

#[test]
fn a_name_the_caller_may_not_read_is_a_403_before_any_search() {
    let (s, b) = rig(true);
    let (status, _) = post(&s, "/_plugins/_ppl", "source=secret");
    assert_eq!(status, 403);
    assert_eq!(b.searches.load(Ordering::SeqCst), 0);
}

#[test]
fn explain_plans_without_searching() {
    let (s, b) = rig(true);
    let (status, v) = post(&s, "/_plugins/_ppl/_explain", "source=accounts | head 3");
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["root"]["description"]["request"]["sourceBuilder"]["size"],
        3
    );
    assert_eq!(b.searches.load(Ordering::SeqCst), 0);
}
