//! docs/specs/data-stream-routes: make, show and remove a data stream by name.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{AliasRecord, DocKind, IndexRecord};
use oss_mem::MemoryBudget;
use oss_meta::{AliasClock, AliasMutations, DeleteMode, DescriptorStore, DocSet, PitClock, Target};
use oss_node::data_stream_route::{DataStreamService, routes};
use oss_node::index_delete::{DeleteService, NoSnapshots, routes as delete_routes};
use oss_node::index_route::{IndexService, routes as index_routes};
use oss_node::index_templates::TemplateSets;
use oss_rest::{Router, Server};
use oss_testkit::budget;

const BUF: usize = 1 << 16;
const TTL: i64 = 30_000;
const ACK: &str = r#"{"acknowledged":true}"#;
const LOGS: &str = r#"{"index_patterns":["logs-*"],"data_stream":{},"template":{"mappings":{"properties":{"msg":{"type":"text"}}}}}"#;
const METRICS: &str =
    r#"{"index_patterns":["metrics-*"],"data_stream":{"timestamp_field":{"name":"ts"}}}"#;
const PLAIN: &str = r#"{"index_patterns":["plain-*"]}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    clock: PitClock,
}

impl Rig {
    fn descriptors(&self) -> DescriptorStore {
        DescriptorStore::new(
            Arc::clone(&self.store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    }
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = || Arc::clone(&store) as Arc<dyn BlobStore>;
    let mut marker = vec![0u8; BUF];
    for (name, source) in [("logs", LOGS), ("metrics", METRICS), ("plain", PLAIN)] {
        DocSet::new(blobs(), "", DocKind::IndexTemplate)
            .expect("set")
            .put(name, source.as_bytes(), false, &mut marker)
            .expect("seeded");
    }
    let clock: PitClock = Arc::new(|| 1_234_567);
    let descriptors = || DescriptorStore::new(blobs(), "", DeleteMode::Conditional).expect("names");
    let sets = TemplateSets::new(
        DocSet::new(blobs(), "", DocKind::IndexTemplate).expect("set"),
        DocSet::new(blobs(), "", DocKind::ComponentTemplate).expect("set"),
    );
    let indices =
        Arc::new(IndexService::new(descriptors(), Arc::clone(&clock), BUF).with_templates(sets));
    let deletes = Arc::new(
        DeleteService::new(
            &blobs(),
            "",
            DeleteMode::Conditional,
            Arc::clone(&clock),
            Arc::new(NoSnapshots),
            BUF,
            2,
        )
        .expect("deletes"),
    );
    let streams = Arc::new(DataStreamService::new(
        Arc::clone(&indices),
        Arc::clone(&deletes),
        descriptors(),
        Arc::clone(&clock),
        BUF,
        TTL,
    ));
    let router = index_routes(Router::new(), indices).expect("routes");
    let router = routes(router, streams).expect("routes");
    let router = delete_routes(router, deletes).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        clock,
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

fn json(reply: &support::Reply) -> serde_json::Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

fn total(r: &Rig) -> u64 {
    [S3Op::Get, S3Op::Put, S3Op::Delete, S3Op::Head, S3Op::List]
        .iter()
        .map(|op| r.store.meter().count(*op))
        .sum()
}

fn index_record(r: &Rig, name: &str) -> Option<(String, Option<String>)> {
    let mut buf = vec![0u8; BUF];
    r.descriptors()
        .get(name, &mut buf)
        .expect("reads")
        .map(|d| (d.uuid.to_string(), d.mapping.as_deref().map(str::to_owned)))
}

/// A stream of `n` backing indices written as the Java writes one, with the
/// alias protocol's hints.
fn seeded(r: &Rig, name: &str, n: i64) {
    let descriptors = r.descriptors();
    let mut buf = vec![0u8; BUF];
    let mut members = Vec::new();
    for generation in 1..=n {
        let index = format!(".ds-{name}-{generation:06}");
        let uuid = format!("uuid-{generation}");
        let record = IndexRecord::new(&index, &uuid, 1);
        descriptors.create(&record, 1, &mut buf).expect("index");
        members.push(index);
    }
    let names: Vec<&str> = members.iter().map(String::as_str).collect();
    let mut stream = AliasRecord::new(name, &names);
    stream.data_stream = true;
    stream.generation = n;
    stream.timestamp_field = Some("@timestamp".into());
    let clock = Arc::clone(&r.clock);
    let now = move || i64::try_from(clock()).unwrap_or(0);
    let pause = |_| {};
    AliasMutations::new(
        &descriptors,
        AliasClock {
            now_millis: &now,
            pause: &pause,
        },
        TTL,
        BUF,
    )
    .create_alias(&stream)
    .expect("stream");
}

// AC-1
#[test]
fn a_stream_is_created_read_back_and_deleted() {
    let r = rig();
    let put = call(&r, "PUT", "/_data_stream/logs-1", None);
    assert_eq!(
        (put.status, put.text().as_str()),
        (200, ACK),
        "{}",
        put.text()
    );
    let (uuid, mapping) = index_record(&r, ".ds-logs-1-000001").expect("backing index");
    let mapping: serde_json::Value =
        serde_json::from_str(&mapping.expect("mapping")).expect("json");
    assert_eq!(mapping["properties"]["@timestamp"]["type"], "date");
    assert_eq!(mapping["properties"]["msg"]["type"], "text");
    let mut buf = vec![0u8; BUF];
    let held = r.descriptors().resolve("logs-1", &mut buf).expect("reads");
    let Target::Alias(record) = held.target else {
        panic!("not an alias");
    };
    assert!(record.data_stream);
    assert_eq!(record.generation, 1);
    assert_eq!(record.timestamp_field.as_deref(), Some("@timestamp"));
    assert_eq!(record.indices.len(), 1);
    let got = call(&r, "GET", "/_data_stream/logs-1", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(
        json(&got),
        serde_json::json!({"data_streams":[{
            "name":"logs-1",
            "timestamp_field":{"name":"@timestamp"},
            "indices":[{"index_name":".ds-logs-1-000001","index_uuid":uuid}],
            "generation":1,
            "status":"GREEN",
            "template":"logs"
        }]})
    );
    let del = call(&r, "DELETE", "/_data_stream/logs-1", None);
    assert_eq!(
        (del.status, del.text().as_str()),
        (200, ACK),
        "{}",
        del.text()
    );
    assert_eq!(call(&r, "GET", "/_data_stream/logs-1", None).status, 404);
    assert!(index_record(&r, ".ds-logs-1-000001").is_none());
}

// AC-1
#[test]
fn the_timestamp_field_is_the_templates() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_data_stream/metrics-1", None).status, 200);
    let (_, mapping) = index_record(&r, ".ds-metrics-1-000001").expect("backing index");
    assert!(
        mapping
            .expect("mapping")
            .contains(r#""ts":{"type":"date"}"#)
    );
    let got = json(&call(&r, "GET", "/_data_stream/metrics-1", None));
    assert_eq!(got["data_streams"][0]["timestamp_field"]["name"], "ts");
}

// AC-2
#[test]
fn what_is_refused_creates_nothing() {
    let r = rig();
    let cases = [
        ("other", 400, "illegal_argument_exception"),
        ("plain-1", 400, "illegal_argument_exception"),
        ("_logs-1", 501, "not_implemented"),
        (".logs-1", 400, "illegal_argument_exception"),
        ("Logs-1", 400, "illegal_argument_exception"),
    ];
    for (name, status, kind) in cases {
        let reply = call(&r, "PUT", &format!("/_data_stream/{name}"), None);
        assert_eq!(reply.status, status, "{name}: {}", reply.text());
        assert_eq!(json(&reply)["error"]["type"], kind, "{name}");
    }
    let none = call(&r, "PUT", "/_data_stream/other", None);
    assert_eq!(
        json(&none)["error"]["reason"],
        "no matching index template found for data stream [other]"
    );
    assert!(index_record(&r, ".ds-other-000001").is_none());
    assert!(index_record(&r, ".ds-plain-1-000001").is_none());
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let again = call(&r, "PUT", "/_data_stream/logs-1", None);
    assert_eq!(again.status, 400);
    assert_eq!(
        json(&again)["error"]["type"],
        "resource_already_exists_exception"
    );
    assert_eq!(call(&r, "PUT", "/logs-2", None).status, 200);
    let taken = call(&r, "PUT", "/_data_stream/logs-2", None);
    assert_eq!(
        json(&taken)["error"]["type"],
        "resource_already_exists_exception"
    );
    assert!(index_record(&r, ".ds-logs-2-000001").is_none());
}

// AC-3
#[test]
fn a_missing_stream_is_404_and_a_lost_backing_index_is_red() {
    let r = rig();
    let missing = call(&r, "GET", "/_data_stream/logs-9", None);
    assert_eq!(missing.status, 404);
    assert_eq!(
        json(&missing)["error"]["type"],
        "resource_not_found_exception"
    );
    assert_eq!(call(&r, "DELETE", "/_data_stream/logs-9", None).status, 404);
    assert_eq!(call(&r, "PUT", "/logs-3", None).status, 200);
    assert_eq!(call(&r, "GET", "/_data_stream/logs-3", None).status, 404);
    assert_eq!(call(&r, "DELETE", "/_data_stream/logs-3", None).status, 404);
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    r.descriptors()
        .delete(".ds-logs-1-000001")
        .expect("deleted");
    let got = json(&call(&r, "GET", "/_data_stream/logs-1", None));
    assert_eq!(got["data_streams"][0]["status"], "RED");
    assert_eq!(got["data_streams"][0]["indices"][0]["index_uuid"], "_na_");
}

// AC-4, AC-5
#[test]
fn a_stream_the_java_wrote_reads_and_deletes_with_every_backing_index() {
    let r = rig();
    seeded(&r, "logs-j", 3);
    let got = json(&call(&r, "GET", "/_data_stream/logs-j", None));
    let indices = got["data_streams"][0]["indices"].as_array().expect("array");
    assert_eq!(indices.len(), 3);
    assert_eq!(indices[2]["index_name"], ".ds-logs-j-000003");
    assert_eq!(indices[2]["index_uuid"], "uuid-3");
    assert_eq!(got["data_streams"][0]["generation"], 3);
    assert_eq!(call(&r, "DELETE", "/_data_stream/logs-j", None).status, 200);
    for generation in 1..=3 {
        assert!(index_record(&r, &format!(".ds-logs-j-{generation:06}")).is_none());
    }
    assert_eq!(call(&r, "GET", "/_data_stream/logs-j", None).status, 404);
}

// AC-6
#[test]
fn the_write_index_of_a_stream_cannot_be_deleted() {
    let r = rig();
    seeded(&r, "logs-w", 2);
    let refused = call(&r, "DELETE", "/.ds-logs-w-000002", None);
    assert_eq!(refused.status, 400, "{}", refused.text());
    assert_eq!(
        json(&refused)["error"]["reason"],
        "index [.ds-logs-w-000002] is the write index for data stream [logs-w] and cannot be deleted"
    );
    assert!(index_record(&r, ".ds-logs-w-000002").is_some());
    assert_eq!(call(&r, "DELETE", "/.ds-logs-w-000001", None).status, 200);
    assert!(index_record(&r, ".ds-logs-w-000001").is_none());
}

// AC-7
#[test]
fn a_pattern_a_list_and_a_listing_are_refused_without_a_request() {
    let r = rig();
    let before = total(&r);
    for (method, target) in [
        ("GET", "/_data_stream/logs-*"),
        ("GET", "/_data_stream/_all"),
        ("GET", "/_data_stream/a,b"),
        ("PUT", "/_data_stream/a,b"),
        ("DELETE", "/_data_stream/logs-*"),
        ("GET", "/_data_stream"),
    ] {
        let reply = call(&r, method, target, None);
        assert_eq!(reply.status, 501, "{method} {target}: {}", reply.text());
    }
    assert_eq!(total(&r), before);
}

// AC-8
#[test]
fn the_requests_are_the_budget_rows() {
    let r = rig();
    let lists = || r.store.meter().count(S3Op::List);
    let start = total(&r);
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let put = total(&r) - start;
    let at = total(&r);
    assert_eq!(call(&r, "GET", "/_data_stream/logs-1", None).status, 200);
    let read = total(&r) - at;
    assert_eq!(lists(), 0, "a put and a get never list");
    budget::check(
        "data_stream.put.requests",
        put,
        "a put with one backing index",
    );
    budget::check(
        "data_stream.get.requests",
        read,
        "a get of a one-index stream",
    );
    // A stream deletes as its one backing index does, plus the record's own steps.
    let at = total(&r);
    assert_eq!(call(&r, "DELETE", "/_data_stream/logs-1", None).status, 200);
    budget::check(
        "data_stream.delete.requests",
        total(&r) - at,
        "a delete of a one-index stream",
    );
    // The write-index guard of `DELETE /{index}` costs a name that begins `.ds-` its reads and no other.
    seeded(&r, "logs-g", 2);
    assert_eq!(call(&r, "PUT", "/plain", None).status, 200);
    let at = total(&r);
    assert_eq!(call(&r, "DELETE", "/plain", None).status, 200);
    let plain = total(&r) - at;
    let at = total(&r);
    assert_eq!(call(&r, "DELETE", "/.ds-logs-g-000001", None).status, 200);
    let guarded = total(&r) - at;
    budget::check(
        "index.delete.write_guard.requests",
        guarded - plain,
        "the guard's reads over a delete of a one-shard index",
    );
}
