//! docs/specs/sql-select and ppl-select AC-7: a SQL or PPL statement over one
//! index costs the requests of the `_search` it pushes, held to
//! docs/request-budgets.md.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, Rounds};
use oss_mem::MemoryBudget;
use oss_node::search::start_reader;
use oss_node::sql_route::{AllowAll, SqlLimits, SqlService, routes};
use oss_rest::{Router, Server};
use oss_testkit::budget;
use serde_json::json;

use crate::support::{budget_config, loaded, request, server_config};

/// Each call is held this long, so calls sent together overlap.
const DELAY: Duration = Duration::from_millis(20);

const SQL: &str = "SELECT title FROM `compat-term` WHERE title = 'brown'";
const PPL: &str = "source=compat-term | where title = 'brown' | fields title";

fn ask(server: &Server, path: &str, statement: &str) -> (u16, String) {
    let body = json!({"query": statement}).to_string();
    let r = request(server.local_addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

/// A parse error, then one cold and one warm run of `statement` on `path`, each
/// held to its budget row.
fn run(keys: [&str; 3], path: &str, statement: &str) {
    let [parse_key, cold_key, warm_key] = keys;
    let store = loaded(Rounds::new(Emulator::new(64 << 20, 1)));
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let memory = MemoryBudget::new(&budget_config());
    let (search_server, service) = start_reader(
        listener,
        server_config(),
        store.clone(),
        "",
        "node-1",
        &memory,
    )
    .expect("starts");
    let sql = Arc::new(SqlService::new(
        Arc::clone(&service) as _,
        Arc::new(AllowAll),
        SqlLimits {
            enabled: true,
            ppl_enabled: true,
            size_limit: 200,
            max_request_bytes: 1 << 16,
            max_buckets: 10_000,
        },
    ));
    let router = routes(Router::new(), &sql).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(listener, server_config(), router, &memory).expect("starts");
    let rounds = store.store();

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    let (status, text) = ask(&server, path, "SELECT * FROM");
    assert_eq!(status, 400, "{text}");
    let (status, text) = ask(&server, path, "source=");
    assert_eq!(status, 400, "{text}");
    budget::check(parse_key, rounds.meter().total() - before, &rounds.detail());

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    let (status, text) = ask(&server, path, statement);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("datarows"), "{text}");
    let detail = rounds.detail();
    budget::check(cold_key, rounds.meter().total() - before, &detail);

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    let (status, text) = ask(&server, path, statement);
    assert_eq!(status, 200, "{text}");
    budget::check(warm_key, rounds.meter().total() - before, &rounds.detail());
    drop(search_server);
}

#[test]
fn a_sql_statement_costs_the_search_it_pushes() {
    run(
        [
            "sql.select.parse_error.requests",
            "sql.select.cold.requests",
            "sql.select.warm.requests",
        ],
        "/_plugins/_sql",
        SQL,
    );
}

#[test]
fn a_ppl_statement_costs_the_search_it_pushes() {
    run(
        [
            "ppl.select.parse_error.requests",
            "ppl.select.cold.requests",
            "ppl.select.warm.requests",
        ],
        "/_plugins/_ppl",
        PPL,
    );
}
