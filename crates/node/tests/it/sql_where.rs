//! docs/specs/sql-scalar-functions AC-4 and AC-5: a function in a SQL `WHERE` runs as a
//! Painless `script` query on the reader, gives the rows its arithmetic says, is refused while
//! the compiler is off, and costs the requests of the plain select.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, Rounds, SliceSource};
use oss_mem::MemoryBudget;
use oss_node::search::start_reader;
use oss_node::sql_route::{AllowAll, SqlLimits, SqlService, routes};
use oss_rest::{Router, Server};
use oss_testkit::index;
use serde_json::{Value, json};

use crate::support::{budget_config, loaded, request, server_config};

const DELAY: Duration = Duration::from_millis(20);

fn ask(server: &Server, statement: &str) -> (u16, Value) {
    let body = json!({"query": statement}).to_string();
    let r = request(
        server.local_addr(),
        "POST",
        "/_plugins/_sql",
        Some(body.as_bytes()),
    );
    let v = serde_json::from_str(&r.text()).unwrap_or(Value::Null);
    (r.status, v)
}

fn ns(v: &Value) -> Vec<i64> {
    let mut out: Vec<i64> = v["datarows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r[0].as_i64().expect("a long"))
        .collect();
    out.sort_unstable();
    out
}

/// A SQL server over a reader on `store`, which holds one built index `nest`.
fn serve(store: &Arc<oss_blob::PooledStore<Rounds<Emulator>>>) -> (Server, Server) {
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
    (search_server, server)
}

/// A fresh store holding the built index `nest`, with a server over a reader on it.
fn fresh() -> (Arc<oss_blob::PooledStore<Rounds<Emulator>>>, Server, Server) {
    let store = loaded(Rounds::new(Emulator::new(64 << 20, 1)));
    for (key, bytes) in index::blobs("", &index::Spec::new("nest", 40)) {
        store
            .put(&key, &mut SliceSource::new(&bytes))
            .expect("put built blob");
    }
    let (reader, server) = serve(&store);
    (store, reader, server)
}

/// The requests a statement costs on a node that has served nothing, then the next one.
fn cost(first: &str, next: &str) -> (u64, u64) {
    let (store, _reader, server) = fresh();
    let rounds = store.store();
    let mut counts = [0; 2];
    for (slot, statement) in counts.iter_mut().zip([first, next]) {
        rounds.reset(DELAY);
        let before = rounds.meter().total();
        let (status, v) = ask(&server, statement);
        assert_eq!(status, 200, "{v}");
        *slot = rounds.meter().total() - before;
    }
    (counts[0], counts[1])
}

/// One test, because the compiler switch is process-wide.
#[test]
fn a_function_in_where_runs_as_a_script_at_the_requests_of_the_plain_select() {
    oss_reader::painless_switch::set_native_compile(true);
    let plain = cost(
        "SELECT n FROM nest WHERE n > 37",
        "SELECT n FROM nest WHERE tag = 't7'",
    );
    let script = cost(
        "SELECT n FROM nest WHERE n + 1 > 38",
        "SELECT n FROM nest WHERE upper(tag) = 'T7'",
    );
    assert!(plain.0 > 0, "a cold search opens the index");
    assert_eq!(plain.1, 0, "a warm search costs nothing");
    assert_eq!(script, plain);

    let (store, _reader, server) = fresh();
    let rounds = store.store();
    let (status, v) = ask(&server, "SELECT n FROM nest WHERE n + 1 > 38");
    assert_eq!(status, 200, "{v}");
    assert_eq!(ns(&v), vec![38, 39]);
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE abs(n - 20) < 3");
    assert_eq!(ns(&v), (18..=22).collect::<Vec<i64>>());
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE n * 2 = 20");
    assert_eq!(ns(&v), vec![10]);
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE n % 10 = 3");
    assert_eq!(ns(&v), vec![3, 13, 23, 33]);
    let want = |f: &dyn Fn(&str) -> bool| -> Vec<i64> {
        (0..40_i64)
            .filter(|i| f(&index::doc_tag(usize::try_from(*i).expect("small"))))
            .collect()
    };
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE length(tag) = 2");
    assert_eq!(ns(&v), want(&|t| t.len() == 2));
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE upper(tag) = 'T7'");
    assert_eq!(ns(&v), want(&|t| t == "t7"));
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE lower(upper(tag)) = 't7'");
    assert_eq!(ns(&v), want(&|t| t == "t7"));
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE concat(tag, '!') = 't7!'");
    assert_eq!(ns(&v), want(&|t| t == "t7"));
    let (_, v) = ask(&server, "SELECT n FROM nest WHERE n > 30 AND n % 2 = 0");
    assert_eq!(ns(&v), vec![32, 34, 36, 38]);

    oss_reader::painless_switch::set_native_compile(false);
    let before = rounds.meter().total();
    let (status, v) = ask(&server, "SELECT n FROM nest WHERE n + 1 > 38");
    assert_eq!(status, 400, "{v}");
    assert!(v.to_string().contains("native_compile"), "{v}");
    assert_eq!(rounds.meter().total() - before, 0);
}
