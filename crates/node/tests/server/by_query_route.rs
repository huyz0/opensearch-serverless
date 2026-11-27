//! docs/specs/node-delete-by-query: delete-by-query over a writer node's own
//! shards, with the search half faked.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use crate::support;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_coordinator::docs::by_query::Hit;
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::by_query::{ByQueryService, ShardMatches};
use oss_node::doc_route::{self, DocService};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{RestError, Router, Server};
use oss_writer::bulk::BulkBudget;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;

struct Lease;

impl EpochLease for Lease {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

/// One index, `docs`, of one shard held here; any other name is unknown.
struct Local;

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local: {}", group.len())
    }
}

/// The search half: the hits the test says match, and the queries asked.
#[derive(Default)]
struct Matches {
    hits: Mutex<Vec<Hit>>,
    queries: Mutex<Vec<String>>,
}

impl ShardMatches for Matches {
    fn newest(&self, _: &str, _: u32, query: &str) -> Result<Option<i64>, RestError> {
        self.queries.lock().unwrap().push(query.to_owned());
        Ok(self.hits.lock().unwrap().iter().map(|h| h.seq_no).max())
    }

    fn page(
        &self,
        _: &str,
        _: u32,
        _: &str,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, RestError> {
        let mut hits: Vec<Hit> = self
            .hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.seq_no <= up_to && after.is_none_or(|a| h.seq_no > a))
            .cloned()
            .collect();
        hits.sort_by_key(|h| h.seq_no);
        hits.truncate(size);
        Ok(hits)
    }
}

struct Rig {
    server: Server,
    matches: Arc<Matches>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log = NodeLogStore::new(store as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Arc::new(Lease)),
        Box::new(Arc::new(Admit)),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)));
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let matches = Arc::new(Matches::default());
    let docs = Arc::new(DocService::new(Arc::clone(&role), [0; 6]));
    let gets = Arc::new(oss_node::get_route::GetService::new(Arc::clone(&role)));
    let by_query = Arc::new(ByQueryService::new(
        role,
        Arc::clone(&matches) as Arc<dyn ShardMatches>,
        BulkBudget::new(1 << 20, 4),
        [0; 6],
    ));
    let router = doc_route::routes(Router::new(), docs).expect("routes");
    let router = oss_node::get_route::routes(router, gets).expect("routes");
    let router = oss_node::by_query::routes(router, by_query).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    Rig { server, matches }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

/// Indexes `id`, and answers the hit a search would have seen.
fn put(r: &Rig, id: &str) -> Hit {
    let reply = call(r, "PUT", &format!("/docs/_doc/{id}"), Some(r#"{"n":1}"#));
    assert!(
        reply.status == 200 || reply.status == 201,
        "{}",
        reply.text()
    );
    let text = reply.text();
    let after = |key: &str| {
        let at = text.find(key).expect(key) + key.len();
        text[at..]
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .expect("digits")
            .parse::<i64>()
            .expect("number")
    };
    Hit {
        id: id.to_owned(),
        seq_no: after(r#""_seq_no":"#),
        primary_term: after(r#""_primary_term":"#),
    }
}

fn gone(r: &Rig, id: &str) -> bool {
    call(r, "DELETE", &format!("/docs/_doc/{id}"), None)
        .text()
        .contains("not_found")
}

const QUERY: &str = r#"{"query":{"match":{"n":1}}}"#;

#[test]
fn the_matching_documents_are_deleted_and_counted() {
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    put(&r, "keep");
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/docs/_delete_by_query", Some(QUERY));
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    for part in [
        r#""timed_out":false,"total":3,"deleted":3,"batches":1,"version_conflicts":0,"noops":0"#,
        r#""failures":[]"#,
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert!(["a", "b", "c"].iter().all(|id| gone(&r, id)));
    assert!(!gone(&r, "keep"), "a document that did not match stays");
    let queries = r.matches.queries.lock().unwrap();
    assert!(queries[0].contains("match"), "{queries:?}");
}

#[test]
fn a_document_rewritten_since_the_search_conflicts() {
    for (conflicts, status, deleted) in [("", 409, 0), ("?conflicts=proceed", 200, 1)] {
        let r = rig();
        let stale = put(&r, "a");
        put(&r, "a");
        let fresh = put(&r, "b");
        *r.matches.hits.lock().unwrap() = vec![stale, fresh];
        let reply = call(
            &r,
            "POST",
            &format!("/docs/_delete_by_query{conflicts}"),
            Some(QUERY),
        );
        let text = reply.text();
        assert_eq!(reply.status, status, "{conflicts}: {text}");
        assert!(text.contains(r#""version_conflicts":1"#), "{text}");
        assert!(text.contains(&format!(r#""deleted":{deleted}"#)), "{text}");
        if status == 409 {
            assert!(text.contains(r#""id":"a""#), "{text}");
            assert!(text.contains("version_conflict_engine_exception"), "{text}");
        } else {
            assert!(gone(&r, "b"), "the walk went on");
        }
    }
}

#[test]
fn max_docs_stops_the_walk() {
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/docs/_delete_by_query?max_docs=2", Some(QUERY));
    assert!(reply.text().contains(r#""deleted":2"#), "{}", reply.text());
    assert!(gone(&r, "a") && gone(&r, "b") && !gone(&r, "c"));
}

#[test]
fn a_request_this_node_cannot_serve_is_refused() {
    let r = rig();
    for (target, body, status, word) in [
        ("/nope/_delete_by_query", QUERY, 404, "index_not_found"),
        ("/docs/_delete_by_query", "{}", 400, "query is missing"),
        ("/docs/_delete_by_query", "{", 400, "failed to parse"),
        (
            "/docs/_delete_by_query?wait_for_completion=false",
            QUERY,
            400,
            "not supported",
        ),
        (
            "/docs/_delete_by_query?conflicts=bad",
            QUERY,
            400,
            "conflicts",
        ),
        ("/docs/_delete_by_query?max_docs=-1", QUERY, 400, "max_docs"),
        (
            "/docs/_delete_by_query?slices=2",
            QUERY,
            400,
            "not supported",
        ),
    ] {
        let reply = call(&r, "POST", target, Some(body));
        assert_eq!(reply.status, status, "{target}: {}", reply.text());
        assert!(reply.text().contains(word), "{target}: {}", reply.text());
    }
}

fn update_query(script: &str) -> String {
    format!(
        r#"{{"query":{{"match":{{"n":1}}}},"script":{{"source":"{script}","params":{{"by":4}}}}}}"#
    )
}

fn source_of(r: &Rig, id: &str) -> String {
    call(r, "GET", &format!("/docs/_doc/{id}"), None).text()
}

// docs/specs/node-update-by-query AC-1, AC-2
#[test]
fn a_script_rewrites_deletes_or_skips_the_matching_documents() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    put(&r, "keep");
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(
        &r,
        "POST",
        "/docs/_update_by_query",
        Some(&update_query(
            "if (ctx._id == 'a') { ctx.op = 'delete' } else if (ctx._id == 'b') { ctx.op = 'noop' } else { ctx._source.n += params.by }",
        )),
    );
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    for part in [
        r#""total":3,"updated":1,"deleted":1,"batches":1,"version_conflicts":0,"noops":1"#,
        r#""failures":[]"#,
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert!(source_of(&r, "a").contains(r#""found":false"#));
    assert!(source_of(&r, "b").contains(r#""n":1"#));
    assert!(source_of(&r, "c").contains(r#""n":5"#));
    assert!(
        source_of(&r, "keep").contains(r#""n":1"#),
        "unmatched stays"
    );
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-update-by-query AC-3, AC-4
#[test]
fn an_update_conflicts_on_a_rewritten_document_and_stops_at_max_docs() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let q = update_query("ctx._source.n += params.by");
    for (target, status, updated) in [
        ("/docs/_update_by_query", 409, 0),
        ("/docs/_update_by_query?conflicts=proceed", 200, 1),
    ] {
        let r = rig();
        let stale = put(&r, "a");
        put(&r, "a");
        let fresh = put(&r, "b");
        *r.matches.hits.lock().unwrap() = vec![stale, fresh];
        let reply = call(&r, "POST", target, Some(&q));
        let text = reply.text();
        assert_eq!(reply.status, status, "{target}: {text}");
        assert!(text.contains(r#""version_conflicts":1"#), "{text}");
        assert!(text.contains(&format!(r#""updated":{updated}"#)), "{text}");
    }
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/docs/_update_by_query?max_docs=2", Some(&q));
    assert!(reply.text().contains(r#""updated":2"#), "{}", reply.text());
    assert!(source_of(&r, "c").contains(r#""n":1"#));
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-update-by-query AC-5
#[test]
fn an_update_by_query_this_node_cannot_serve_is_refused() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    let r = rig();
    oss_reader::painless_switch::set_native_compile(false);
    let off = call(
        &r,
        "POST",
        "/docs/_update_by_query",
        Some(&update_query("1")),
    );
    assert_eq!(off.status, 501, "{}", off.text());
    oss_reader::painless_switch::set_native_compile(true);
    let match_all = r#"{"query":{"match_all":{}}}"#.to_owned();
    let stored = r#"{"script":{"id":"s"}}"#.to_owned();
    for (target, body, word) in [
        (
            "/nope/_update_by_query",
            update_query("1"),
            "index_not_found",
        ),
        ("/docs/_update_by_query", match_all, "[script] is required"),
        (
            "/docs/_update_by_query?slices=2",
            update_query("1"),
            "not supported",
        ),
        ("/docs/_update_by_query", stored, "[script]"),
    ] {
        let reply = call(&r, "POST", target, Some(&body));
        assert!(
            reply.status == 400 || reply.status == 404,
            "{target}: {}",
            reply.text()
        );
        assert!(reply.text().contains(word), "{target}: {}", reply.text());
    }
    // A script that does not compile, or throws, fails the request with its error.
    let hits = vec![put(&r, "a")];
    *r.matches.hits.lock().unwrap() = hits;
    for script in ["ctx._source.n = ", "ctx._source.n = ctx.nope.x"] {
        let reply = call(
            &r,
            "POST",
            "/docs/_update_by_query",
            Some(&update_query(script)),
        );
        assert_eq!(reply.status, 400, "{script}: {}", reply.text());
        assert!(
            reply.text().contains("script_exception"),
            "{}",
            reply.text()
        );
    }
    oss_reader::painless_switch::set_native_compile(false);
}
