//! docs/specs/async-search slice 3: the routes over a counting backend (AC-1,
//! AC-2, AC-3, AC-4 and the routes themselves).

use crate::support;

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::async_search::{SearchRunner, Service, routes};
use oss_node::jobs::{Jobs, Limits};
use oss_node::sql_route::{SqlBackend, SqlFailure};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

#[derive(Default)]
struct Backend {
    searches: AtomicUsize,
    held: Mutex<bool>,
    wake: Condvar,
}

impl Backend {
    fn open(&self) {
        *self.held.lock().expect("gate") = true;
        self.wake.notify_all();
    }
}

impl SqlBackend for Backend {
    fn mapping(&self, _: &str) -> Result<Value, SqlFailure> {
        Ok(json!({}))
    }

    fn search(&self, names: &[String], body: &Value) -> Result<Vec<u8>, SqlFailure> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        if names == ["missing"] {
            return Err(SqlFailure::Body(
                404,
                json!({"error": {"type": "index_not_found_exception", "reason": "no such index [missing]"}, "status": 404})
                    .to_string()
                    .into_bytes(),
            ));
        }
        if body["query"]["match"].get("slow").is_some() {
            let mut open = self.held.lock().expect("gate");
            while !*open {
                open = self.wake.wait(open).expect("gate");
            }
        }
        Ok(
            json!({"took": 1, "hits": {"hits": [], "total": {"value": 0, "relation": "eq"}}})
                .to_string()
                .into_bytes(),
        )
    }
}

struct Rig {
    server: Server,
    backend: Arc<Backend>,
}

fn rig() -> Rig {
    let backend = Arc::new(Backend::default());
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::RequestBodies, 16 << 20)
        .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
        .and_then(|c| c.with_pool(Pool::Extensions, 16 << 20))
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let jobs = Jobs::new(
        "n1",
        &budget,
        Limits::default(),
        Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        }),
    );
    let service = Arc::new(Service::new(
        jobs,
        Arc::new(SearchRunner::new(Arc::clone(&backend) as _)),
        "opensearch",
        "n1",
    ));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, backend }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<Value>) -> (u16, Value) {
    let bytes = body.map(|b| b.to_string().into_bytes());
    let reply = support::request(r.server.local_addr(), method, target, bytes.as_deref());
    (
        reply.status,
        serde_json::from_slice(&reply.body).unwrap_or(Value::Null),
    )
}

const ROOT: &str = "/_plugins/_asynchronous_search";

// AC-1, AC-4: a search that finishes inside the wait is CLOSED with an id, and
// the id is gone after; the legacy prefix and an index-leading path serve it.
#[test]
fn a_finished_search_is_closed_then_gone_on_every_path() {
    let r = rig();
    for target in [
        ROOT.to_owned(),
        "/_opendistro/_asynchronous_search".to_owned(),
        format!("/logs{ROOT}"),
    ] {
        let (status, v) = call(
            &r,
            "POST",
            &target,
            Some(json!({"query": {"match_all": {}}})),
        );
        assert_eq!((status, v["state"].as_str()), (200, Some("CLOSED")), "{v}");
        let id = v["id"].as_str().expect("id").to_owned();
        assert_eq!(call(&r, "GET", &format!("{ROOT}/{id}"), None).0, 404);
    }
    assert_eq!(r.backend.searches.load(Ordering::SeqCst), 3);
}

// AC-4: kept is PERSISTING, then STORE_RESIDENT until deleted.
#[test]
fn a_kept_search_is_persisting_then_store_resident_until_deleted() {
    let r = rig();
    let (_, v) = call(
        &r,
        "POST",
        &format!("{ROOT}?keep_on_completion=true"),
        Some(json!({})),
    );
    assert_eq!(v["state"], "PERSISTING");
    let id = v["id"].as_str().expect("id").to_owned();
    let (status, got) = call(&r, "GET", &format!("{ROOT}/{id}"), None);
    assert_eq!(
        (status, got["state"].as_str()),
        (200, Some("STORE_RESIDENT"))
    );
    let (status, ack) = call(&r, "DELETE", &format!("{ROOT}/{id}"), None);
    assert_eq!((status, ack), (200, json!({"acknowledged": true})));
    assert_eq!(call(&r, "GET", &format!("{ROOT}/{id}"), None).0, 404);
}

// AC-2: a running search is RUNNING on submit and get, and finishes kept.
#[test]
fn a_running_search_is_running_until_it_finishes() {
    let r = rig();
    let target = format!("{ROOT}?keep_on_completion=true&wait_for_completion_timeout=1ms");
    let (_, v) = call(
        &r,
        "POST",
        &target,
        Some(json!({"query": {"match": {"slow": "yes"}}})),
    );
    assert_eq!(v["state"], "RUNNING", "{v}");
    let id = v["id"].as_str().expect("id").to_owned();
    assert_eq!(
        call(&r, "GET", &format!("{ROOT}/{id}"), None).1["state"],
        "RUNNING"
    );
    r.backend.open();
    let (_, done) = call(
        &r,
        "GET",
        &format!("{ROOT}/{id}?wait_for_completion_timeout=5s"),
        None,
    );
    assert_eq!(done["state"], "STORE_RESIDENT", "{done}");
}

// AC-3: an execution error is 200 CLOSED with the error; stats answer.
#[test]
fn an_execution_error_is_a_closed_search_with_the_error() {
    let r = rig();
    let (status, v) = call(&r, "POST", &format!("/missing{ROOT}"), Some(json!({})));
    assert_eq!((status, v["state"].as_str()), (200, Some("CLOSED")), "{v}");
    assert_eq!(v["error"]["type"], "index_not_found_exception");
    assert_eq!(call(&r, "GET", &format!("{ROOT}/stats"), None).0, 200);
}

// AC-13: on an assembled node, with no extension pool, a kept search is
// held, and a read, a delete and the stats cost the object store nothing.
#[cfg(all(feature = "reader", feature = "writer"))]
#[test]
fn reads_deletes_and_stats_make_no_object_store_request() {
    use oss_blob::BlobStore;
    let (r, store) = super::assemble::start(256 << 20);
    let call = |method: &str, target: &str, body: Option<&str>| {
        let reply = super::assemble::call(&r, method, target, body);
        let value: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
        (reply.status, value)
    };
    let (status, v) = call(
        "POST",
        &format!("{ROOT}?keep_on_completion=true"),
        Some("{}"),
    );
    assert_eq!(
        (status, v["state"].as_str()),
        (200, Some("PERSISTING")),
        "{v}"
    );
    let id = v["id"].as_str().expect("id").to_owned();
    let before = store.meter().total();
    assert_eq!(call("GET", &format!("{ROOT}/{id}"), None).0, 200);
    assert_eq!(call("GET", &format!("{ROOT}/stats"), None).0, 200);
    assert_eq!(call("DELETE", &format!("{ROOT}/{id}"), None).0, 200);
    oss_testkit::budget::check(
        "async_search.read.requests",
        store.meter().total() - before,
        "",
    );
}
