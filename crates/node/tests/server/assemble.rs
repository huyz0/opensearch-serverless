//! docs/specs/node-assembly: a node assembled from its roles.

#![cfg(all(feature = "reader", feature = "writer"))]

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{MAX_POINT_IN_TIME_BYTES, NodeLogEpoch, ShardKey};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::assemble::{AssembleError, Config, Running, Writer, assemble};
use oss_node::writer::Routes;
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

struct Local;

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        Vec::new()
    }
}

fn budget(request_bodies: usize, open_indexes: usize) -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(512 << 20)
            .with_pool(Pool::RequestBodies, request_bodies)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, open_indexes))
            .and_then(|c| c.with_pool(Pool::Transport, 8 << 20))
            .expect("fits"),
    )
}

fn config() -> Config {
    Config {
        publish_every: Duration::from_millis(10),
        publish_age_millis: 0,
        ..Config::new("", "node-1", [0; 6])
    }
}

fn writer(store: &Arc<Emulator>) -> Writer {
    let log =
        NodeLogStore::new(Arc::clone(store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
    Writer {
        committer: Arc::new(NodeLogCommitter::new(
            log,
            7,
            Box::new(Arc::new(Lease)),
            Box::new(Arc::new(Admit)),
            CommitterConfig {
                window: Duration::from_millis(0),
                ..CommitterConfig::default()
            },
        )),
        routes: Arc::new(Local),
        opener: None,
        names: None,
        incarnation: None,
        buckets: None,
    }
}

fn start(request_bodies: usize) -> (Running, Arc<Emulator>) {
    start_with(config(), request_bodies)
}

fn start_with(config: Config, request_bodies: usize) -> (Running, Arc<Emulator>) {
    let store = Arc::new(Emulator::new(1 << 28, 9));
    let budget = budget(request_bodies, 16 << 20);
    let node = assemble(
        &config,
        Arc::clone(&store) as Arc<dyn BlobStore>,
        &budget,
        writer(&store),
    )
    .expect("assembles");
    node.role
        .open_shard(
            &info(),
            0,
            3,
            Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
        )
        .expect("opens");
    let running = node
        .start(
            TcpListener::bind("127.0.0.1:0").expect("binds"),
            support::server_config(),
            &budget,
        )
        .expect("starts");
    (running, store)
}

fn call(r: &Running, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(r.addr(), method, target, body.map(str::as_bytes))
}

#[test]
fn every_route_family_is_registered_on_the_one_listener() {
    let (r, _) = start(256 << 20);
    let wrote = call(&r, "PUT", "/docs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    let bulk = call(
        &r,
        "POST",
        "/docs/_bulk",
        Some("{\"index\":{\"_id\":\"2\"}}\n{\"n\":2}\n"),
    );
    assert_eq!(bulk.status, 200, "{}", bulk.text());
    let search = call(&r, "POST", "/docs/_search", Some("{}"));
    assert_eq!(search.status, 404, "{}", search.text());
    assert!(
        search.text().contains("index_not_found"),
        "{}",
        search.text()
    );
    let pit = call(&r, "POST", "/docs/_pit", None);
    assert_eq!(pit.status, 404, "{}", pit.text());
    assert!(pit.text().contains("index_not_found"), "{}", pit.text());
    let list = call(&r, "GET", "/_search/point_in_time/_all", None);
    assert_eq!(list.status, 200, "{}", list.text());
    #[cfg(feature = "coordinator")]
    {
        let made = call(&r, "PUT", "/fresh", None);
        assert_eq!(made.status, 200, "{}", made.text());
        assert_eq!(call(&r, "HEAD", "/fresh", None).status, 200);
        assert_eq!(call(&r, "HEAD", "/never", None).status, 404);
    }
    let unregistered = call(&r, "GET", "/_nothing/here/at/all", None);
    assert_ne!(unregistered.status, 200);
}

#[test]
fn the_role_is_the_one_the_routes_write_through_and_the_timer_publishes_it() {
    let (r, _) = start(256 << 20);
    assert!(r.role.held("docs-uuid", 0));
    assert!(r.role.newest_commit("docs-uuid", 0).is_none());
    assert_eq!(
        call(&r, "PUT", "/docs/_doc/1", Some(r#"{"n":1}"#)).status,
        201
    );
    let started = Instant::now();
    while r.role.newest_commit("docs-uuid", 0).is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timer published nothing"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_pit_buffer_is_charged_to_the_request_body_pool() {
    assert_eq!(
        Config::new("", "n", [0; 6]).pit_buffer_bytes,
        MAX_POINT_IN_TIME_BYTES
    );
    // A pool that cannot hold one view's buffer refuses the create at once.
    let (r, store) = start(MAX_POINT_IN_TIME_BYTES / 2);
    let before = store.meter().count(S3Op::Put);
    let refused = call(&r, "POST", "/docs/_pit", None);
    assert_eq!(refused.status, 429, "{}", refused.text());
    assert_eq!(store.meter().count(S3Op::Put), before, "nothing written");
}

#[test]
fn a_budget_with_no_open_indexes_is_refused() {
    let store = Arc::new(Emulator::new(1 << 20, 1));
    let refused = assemble(
        &config(),
        Arc::clone(&store) as Arc<dyn BlobStore>,
        &budget(64 << 20, 0),
        writer(&store),
    );
    assert!(matches!(refused, Err(AssembleError::NoOpenIndexes)));
}

/// A node whose janitor runs every 10 ms, written to, published, and with a
/// blob of an older term nothing names.
fn with_an_orphan(sweep_dry_run: Option<bool>) -> (Running, Arc<Emulator>, String) {
    let mut config = Config {
        janitor_every: Duration::from_millis(10),
        ..config()
    };
    config.sweep.minimum_unreferenced_millis = 0;
    if let Some(dry_run) = sweep_dry_run {
        config.sweep.dry_run = dry_run;
    }
    let (r, store) = start_with(config, 256 << 20);
    // Planted before the first publish: the pass sweeps a shard once for
    // each commit it publishes and then only while it watches a blob, so a
    // blob planted after that first sweep would never be seen.
    let key = "segments/docs#docs-uuid#0/t=1/orphan".to_owned();
    store
        .put(&key, &mut SliceSource::new(b"old"), &mut [0u8; 16])
        .expect("planted");
    assert_eq!(
        call(&r, "PUT", "/docs/_doc/1", Some(r#"{"n":1}"#)).status,
        201
    );
    let started = Instant::now();
    while r.role.newest_commit("docs-uuid", 0).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "not published");
        std::thread::sleep(Duration::from_millis(5));
    }
    (r, store, key)
}

#[test]
fn the_janitor_timer_sweeps_the_shards_the_node_holds() {
    let (_r, store, key) = with_an_orphan(Some(false));
    let started = Instant::now();
    while store.head(&key).expect("heads").is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the janitor deleted nothing"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_node_starts_with_its_janitor_live() {
    assert!(!Config::new("", "n", [0; 6]).sweep.dry_run);
    let (_r, store, key) = with_an_orphan(None);
    let started = Instant::now();
    while store.head(&key).expect("heads").is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the janitor deleted nothing"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_janitor_set_to_a_dry_run_counts_and_deletes_nothing() {
    let (_r, store, key) = with_an_orphan(Some(true));
    // Two passes have listed the points in time and the shard each.
    let lists = store.meter().count(S3Op::List);
    let started = Instant::now();
    while store.meter().count(S3Op::List) < lists + 4 {
        assert!(started.elapsed() < Duration::from_secs(10), "no passes");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        store.head(&key).expect("heads").is_some(),
        "dry run kept it"
    );
}

#[cfg(feature = "coordinator")]
#[test]
fn a_delete_by_query_removes_what_the_reader_finds_through_the_writer() {
    let (r, store) = start(256 << 20);
    store
        .create_register(
            "indices/docs",
            br#"{"name":"docs","uuid":"docs-uuid","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"}}}","settings":{}}"#,
        )
        .expect("descriptor");
    // One bulk is one commit, so the first publish holds all four.
    let lines: String = (1..=4)
        .map(|n| format!("{{\"index\":{{\"_id\":\"{n}\"}}}}\n{{\"n\":{n}}}\n"))
        .collect();
    let put = call(&r, "POST", "/docs/_bulk", Some(&lines));
    assert_eq!(put.status, 200, "{}", put.text());
    let started = Instant::now();
    while r.role.newest_commit("docs-uuid", 0).is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timer published nothing"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let gone = call(
        &r,
        "POST",
        "/docs/_delete_by_query?refresh=true",
        Some(r#"{"query":{"range":{"n":{"gte":3}}}}"#),
    );
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert!(gone.text().contains("\"deleted\":2"), "{}", gone.text());
    assert!(gone.text().contains("\"total\":2"), "{}", gone.text());
    for id in ["1", "2"] {
        assert_eq!(
            call(&r, "GET", &format!("/docs/_doc/{id}"), None).status,
            200
        );
    }
    for id in ["3", "4"] {
        assert_eq!(
            call(&r, "GET", &format!("/docs/_doc/{id}"), None).status,
            404
        );
    }
}

/// Whether the shard `docs` holds is hibernated within a while of its first write.
fn hibernates_after(hibernate_after_millis: u64) -> bool {
    let (r, _) = start_with(
        Config {
            hibernate_after_millis,
            ..config()
        },
        256 << 20,
    );
    assert_eq!(
        call(&r, "PUT", "/docs/_doc/1", Some(r#"{"n":1}"#)).status,
        201
    );
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1500) {
        if r.role.asleep("docs-uuid", 0) {
            return !r.role.held("docs-uuid", 0);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

// docs/specs/writer-hibernation AC-9
#[test]
fn the_publish_timer_hibernates_an_idle_published_shard_only_when_the_threshold_is_set() {
    assert!(hibernates_after(100), "an idle shard sleeps on the timer");
    assert!(!hibernates_after(0), "unset, a shard is never hibernated");
}

#[test]
fn the_snapshot_routes_are_served_only_when_snapshots_are_on() {
    let (off, _) = start(256 << 20);
    assert_ne!(call(&off, "GET", "/_snapshot", None).status, 200);
    assert_ne!(call(&off, "PUT", "/_snapshot/r", None).status, 501);
    let on = Config {
        snapshots: true,
        ..config()
    };
    let (r, _) = start_with(on, 256 << 20);
    assert_eq!(call(&r, "GET", "/_snapshot", None).status, 200);
    assert_eq!(call(&r, "PUT", "/_snapshot/r", None).status, 501);
}

#[test]
fn the_template_routes_are_served_only_when_templates_are_on() {
    let (off, _) = start(256 << 20);
    assert_ne!(call(&off, "GET", "/_index_template", None).status, 200);
    let on = Config {
        index_templates: true,
        ..config()
    };
    let (r, _) = start_with(on, 256 << 20);
    let body = r#"{"index_patterns":["logs-*"]}"#;
    assert_eq!(
        call(&r, "PUT", "/_index_template/logs", Some(body)).status,
        200
    );
    #[cfg(feature = "coordinator")]
    {
        let made = call(&r, "PUT", "/logs-1", None);
        assert_eq!(made.status, 200, "{}", made.text());
    }
    let got = call(&r, "GET", "/_index_template/logs", None);
    assert!(got.text().contains("logs-*"), "{}", got.text());
}

#[test]
fn the_pipeline_routes_are_served_only_when_pipelines_are_on() {
    let (off, _) = start(256 << 20);
    assert_ne!(call(&off, "GET", "/_ingest/pipeline", None).status, 200);
    let on = Config {
        ingest_pipelines: true,
        ..config()
    };
    let (r, _) = start_with(on, 256 << 20);
    let body = r#"{"processors":[{"trim":{"field":"a"}}]}"#;
    assert_eq!(
        call(&r, "PUT", "/_ingest/pipeline/p", Some(body)).status,
        200
    );
    let got = call(&r, "GET", "/_ingest/pipeline/p", None);
    assert!(got.text().contains("trim"), "{}", got.text());
}
