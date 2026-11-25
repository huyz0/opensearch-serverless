//! `_search/pipeline` over a node's document set, and the pipelines a search
//! reads from it (docs/specs/search-pipelines/SPEC.md).

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::search::PipelineSource;
use oss_node::search_pipeline_route::{SearchPipelines, routes};
use oss_rest::{Router, Server};
use oss_testkit::budget;

const BUF: usize = 1 << 16;
const PAIR: &str = r#"{"description":"d","request_processors":[{"oversample":{"sample_factor":2}}],"response_processors":[{"truncate_hits":{}}]}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    pipelines: Arc<SearchPipelines>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let pipelines = SearchPipelines::open(&blobs, "", BUF).expect("opens");
    let router = routes(Router::new(), &pipelines).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        pipelines,
    }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}

// AC-4
#[test]
fn a_pipeline_is_stored_read_listed_replaced_and_removed() {
    let r = rig();
    let put = call(&r, "PUT", "/_search/pipeline/p1", Some(PAIR));
    assert_eq!(
        (put.status, put.text()),
        (200, r#"{"acknowledged":true}"#.to_owned())
    );
    assert_eq!(
        call(&r, "PUT", "/_search/pipeline/p2", Some(PAIR)).status,
        200
    );
    assert_eq!(
        call(&r, "GET", "/_search/pipeline/p1", None).text(),
        format!(r#"{{"p1":{PAIR}}}"#)
    );
    let all = call(&r, "GET", "/_search/pipeline", None).text();
    assert_eq!(all, format!(r#"{{"p1":{PAIR},"p2":{PAIR}}}"#));
    assert_eq!(call(&r, "GET", "/_search/pipeline/*", None).text(), all);
    let other = r#"{"response_processors":[{"truncate_hits":{"target_size":1}}]}"#;
    assert_eq!(
        call(&r, "PUT", "/_search/pipeline/p1", Some(other)).status,
        200
    );
    assert_eq!(
        call(&r, "GET", "/_search/pipeline/p1", None).text(),
        format!(r#"{{"p1":{other}}}"#)
    );
    assert_eq!(call(&r, "DELETE", "/_search/pipeline/p1", None).status, 200);
    let gone = call(&r, "DELETE", "/_search/pipeline/p1", None);
    assert_eq!(gone.status, 404);
    assert!(gone.text().contains("pipeline_missing"), "{}", gone.text());
    assert_eq!(call(&r, "GET", "/_search/pipeline/p1", None).status, 404);
}

// AC-3
#[test]
fn a_mutation_names_one_concrete_id_and_only_put_stores() {
    let r = rig();
    for id in ["_none", "a*", "a,b"] {
        let put = call(&r, "PUT", &format!("/_search/pipeline/{id}"), Some(PAIR));
        assert_eq!(put.status, 501, "{id}: {}", put.text());
        assert_eq!(
            call(&r, "DELETE", &format!("/_search/pipeline/{id}"), None).status,
            501
        );
    }
    assert_ne!(
        call(&r, "POST", "/_search/pipeline/p", Some(PAIR)).status,
        200
    );
    assert_eq!(call(&r, "GET", "/_search/pipeline/a,b", None).status, 501);
    assert_eq!(
        call(&r, "GET", "/_search/pipeline/none-such", None).status,
        404
    );
}

// AC-1, AC-2
#[test]
fn what_the_node_cannot_run_is_refused_and_stores_nothing() {
    let r = rig();
    let collapse = r#"{"response_processors":[{"collapse":{"field":"a"}}]}"#;
    let refused = call(&r, "PUT", "/_search/pipeline/p", Some(collapse));
    assert_eq!(refused.status, 501, "{}", refused.text());
    assert!(refused.text().contains("[collapse]"), "{}", refused.text());
    let unknown = call(
        &r,
        "PUT",
        "/_search/pipeline/p",
        Some(r#"{"request_processors":[{"nope":{}}]}"#),
    );
    assert_eq!(unknown.status, 400, "{}", unknown.text());
    assert!(
        unknown.text().contains("invalid_pipeline"),
        "{}",
        unknown.text()
    );
    assert_eq!(call(&r, "PUT", "/_search/pipeline/p", None).status, 400);
    assert_eq!(call(&r, "GET", "/_search/pipeline/p", None).status, 404);
}

// AC-11
#[test]
fn a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_search/pipeline/p1", Some(PAIR)).status,
        200
    );
    assert_eq!(call(&r, "GET", "/_search/pipeline", None).status, 200);
    let before = requests(&r);
    assert_eq!(call(&r, "GET", "/_search/pipeline", None).status, 200);
    assert_eq!(requests(&r) - before, 1, "one marker GET");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

// AC-11
#[test]
fn a_search_resolves_its_pipeline_with_one_marker_get_and_no_list() {
    use crate::assemble::{call, config, start_with};
    use oss_node::assemble::Config;
    let on = Config {
        search_pipelines: true,
        ..config()
    };
    let (r, store) = start_with(on, 256 << 20);
    let total = || -> u64 { S3Op::ALL.iter().map(|op| store.meter().count(*op)).sum() };
    let spent = |target: &str| {
        let before = total();
        call(&r, "POST", target, Some("{}"));
        total() - before
    };
    assert_eq!(
        call(&r, "PUT", "/_search/pipeline/p1", Some(PAIR)).status,
        200
    );
    // The first named search reads the set; the next finds it unchanged.
    spent("/docs/_search?search_pipeline=p1");
    let named = spent("/docs/_search?search_pipeline=p1");
    let plain = spent("/docs/_search");
    budget::check(
        "search_pipeline.resolve.requests",
        named - plain,
        "a named search against a plain one",
    );
    assert_eq!(store.meter().count(S3Op::List), 0);
}

#[test]
fn resolving_reads_the_marker_and_reports_a_missing_pipeline() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/_search/pipeline/p1", Some(PAIR)).status,
        200
    );
    r.pipelines.resolve("p1").expect("resolves");
    assert!(matches!(
        r.pipelines.resolve("nope"),
        Err(oss_node::search::PipelineFault::Missing)
    ));
    assert_eq!(r.pipelines.reserve_bytes(), BUF * 2);
}

// AC-11
#[test]
fn a_search_naming_no_pipeline_makes_no_request_for_the_set() {
    use crate::assemble::{call, config, start_with};
    use oss_node::assemble::Config;
    let on = Config {
        search_pipelines: true,
        ..config()
    };
    let (r, store) = start_with(on, 256 << 20);
    let total = || -> u64 { S3Op::ALL.iter().map(|op| store.meter().count(*op)).sum() };
    let spent = |target: &str| {
        let before = total();
        call(&r, "POST", target, Some("{}"));
        total() - before
    };
    spent("/docs/_search");
    let plain = spent("/docs/_search");
    let none = spent("/docs/_search?search_pipeline=_none");
    budget::check(
        "search_pipeline.none.requests",
        none.abs_diff(plain),
        "_none against a plain search",
    );
    assert_eq!(store.meter().count(S3Op::List), 0);
}

// AC-5
#[test]
fn the_routes_and_the_parameter_exist_only_when_search_pipelines_are_on() {
    use crate::assemble::{call, config, start, start_with};
    use oss_node::assemble::Config;
    let (off, _) = start(256 << 20);
    assert_ne!(call(&off, "GET", "/_search/pipeline", None).status, 200);
    let refused = call(&off, "POST", "/docs/_search?search_pipeline=p", Some("{}"));
    assert_eq!(refused.status, 501, "{}", refused.text());
    assert_eq!(
        call(
            &off,
            "POST",
            "/docs/_search?search_pipeline=_none",
            Some("{}")
        )
        .status,
        404
    );
    let on = Config {
        search_pipelines: true,
        ..config()
    };
    let (r, _) = start_with(on, 256 << 20);
    let missing = call(&r, "POST", "/docs/_search?search_pipeline=p", Some("{}"));
    assert_eq!(missing.status, 400, "{}", missing.text());
    assert!(
        missing.text().contains("pipeline_missing"),
        "{}",
        missing.text()
    );
    assert_eq!(
        call(&r, "PUT", "/_search/pipeline/p", Some(PAIR)).status,
        200
    );
    // The pipeline resolved; what is left to fail is the index, which this node has no commit of.
    let found = call(&r, "POST", "/docs/_search?search_pipeline=p", Some("{}"));
    assert_eq!(found.status, 404, "{}", found.text());
    assert!(found.text().contains("index_not_found"), "{}", found.text());
}
