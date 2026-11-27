//! docs/specs/ltr AC-10 and AC-13: the model cache's reads, freshness and
//! bounds, over the emulator with a mocked-by-sleep clock.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::ltr_route::{LtrService, ModelCache, StoreModels, routes};
use oss_reader::ltr::models::{Models, ResolveError};
use oss_rest::{Router, Server};

const SET: &str = r#"{"featureset":{"features":[{"name":"a","params":["q"],"template":{"match":{"t":"{{q}}"}}},{"name":"b","template":{"match_all":{}}}]}}"#;
const MODEL: &str =
    r#"{"model":{"name":"m","model":{"type":"model/linear","definition":"{\"a\":2,\"b\":1}"}}}"#;
const TTL: Duration = Duration::from_millis(300);

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    models: Arc<StoreModels>,
    budget: MemoryBudget,
}

fn rig(cache_bytes: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let cache = ModelCache {
        buffer_bytes: 1 << 16,
        max_window: 77,
        ttl: TTL,
        bytes: cache_bytes,
    };
    let pool = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::OpenIndexes, 64 << 20)
            .expect("fits"),
    );
    let models = Arc::new(StoreModels::new(&blobs, "", cache, pool.clone()));
    let service =
        Arc::new(LtrService::new(&blobs, "", 1 << 16).with_cache(Arc::clone(&models), "n1"));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        models,
        budget: pool,
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

fn gets(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Get)
}

fn seed(r: &Rig) {
    assert_eq!(call(r, "PUT", "/_ltr", None).status, 200);
    assert_eq!(
        call(r, "PUT", "/_ltr/_featureset/fs", Some(SET)).status,
        201
    );
    assert_eq!(
        call(r, "POST", "/_ltr/_featureset/fs/_createmodel", Some(MODEL)).status,
        201
    );
}

// AC-10
#[test]
fn a_cold_resolve_reads_the_store_a_warm_one_does_not_and_an_old_one_reads_the_marker() {
    let r = rig(8 << 20);
    seed(&r);
    let before = gets(&r);
    let m = r.models.resolve(None, "m").expect("resolves");
    assert_eq!(m.features.len(), 2);
    assert_eq!(gets(&r) - before, 3, "a cold resolve");
    let before = gets(&r);
    for _ in 0..100 {
        r.models.resolve(None, "m").expect("resolves");
    }
    assert_eq!(gets(&r) - before, 0, "a warm resolve");
    std::thread::sleep(TTL + Duration::from_millis(50));
    let before = gets(&r);
    r.models.resolve(None, "m").expect("resolves");
    assert_eq!(gets(&r) - before, 1, "an old entry reads the marker once");
    let before = gets(&r);
    r.models.resolve(None, "m").expect("resolves");
    assert_eq!(gets(&r) - before, 0, "and is fresh again");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

// AC-10
#[test]
fn a_thousand_searches_cost_one_load_and_a_marker_read_a_ttl() {
    let r = rig(8 << 20);
    seed(&r);
    let before = gets(&r);
    let start = std::time::Instant::now();
    for _ in 0..1_000 {
        r.models.resolve(None, "m").expect("resolves");
        std::thread::sleep(Duration::from_millis(1));
    }
    let windows = start.elapsed().as_millis() / TTL.as_millis() + 1;
    let spent = u128::from(gets(&r) - before);
    assert!(
        spent <= 3 + windows,
        "{spent} GETs over {windows} TTL windows, want a load and one marker read each"
    );
    assert!(spent >= 4, "the run must outlast a TTL: {spent}");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

// AC-11
#[test]
fn a_feature_set_is_read_cached_and_kept_apart_from_a_model_of_its_name() {
    let r = rig(8 << 20);
    seed(&r);
    let before = gets(&r);
    let set = r.models.feature_set(None, "fs").expect("resolves");
    assert_eq!(
        set.features
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(set.features[0].params, ["q"]);
    assert_eq!(gets(&r) - before, 3, "a cold read");
    let before = gets(&r);
    r.models.feature_set(None, "fs").expect("resolves");
    assert_eq!(gets(&r) - before, 0, "a warm read");
    assert!(matches!(
        r.models.feature_set(None, "m"),
        Err(ResolveError::NotFound(_))
    ));
    assert!(matches!(
        r.models.resolve(None, "fs"),
        Err(ResolveError::NotFound(_))
    ));
}

// AC-10
#[test]
fn a_model_replaced_through_the_store_is_seen_after_the_ttl() {
    let r = rig(8 << 20);
    seed(&r);
    let first = r.models.resolve(None, "m").expect("resolves");
    let swapped = MODEL.replace(r#"\"a\":2"#, r#"\"a\":10"#);
    assert_eq!(
        call(
            &r,
            "POST",
            "/_ltr/_featureset/fs/_createmodel",
            Some(&swapped)
        )
        .status,
        200
    );
    // before the ttl: the cached one
    let same = r.models.resolve(None, "m").expect("resolves");
    assert!(Arc::ptr_eq(&first, &same));
    std::thread::sleep(TTL + Duration::from_millis(50));
    let now = r.models.resolve(None, "m").expect("resolves");
    assert!(!Arc::ptr_eq(&first, &now));
    assert_eq!(now.evaluator.score(&[1.0, 0.0]), 10.0);
}

#[test]
fn a_missing_store_or_model_is_not_found_and_the_window_is_the_settings() {
    let r = rig(8 << 20);
    assert!(matches!(
        r.models.resolve(None, "m"),
        Err(ResolveError::NotFound(_))
    ));
    seed(&r);
    assert!(matches!(
        r.models.resolve(None, "nope"),
        Err(ResolveError::NotFound(_))
    ));
    assert!(matches!(
        r.models.resolve(Some("other"), "m"),
        Err(ResolveError::NotFound(_))
    ));
    assert_eq!(r.models.max_window(), 77);
}

// AC-13
#[test]
fn what_the_cache_holds_is_reserved_bounded_and_released() {
    let r = rig(8 << 20);
    seed(&r);
    let free = r.budget.available(Pool::OpenIndexes);
    r.models.resolve(None, "m").expect("resolves");
    let (n, bytes) = r.models.held();
    assert_eq!(n, 1);
    assert_eq!(
        free - r.budget.available(Pool::OpenIndexes),
        bytes,
        "the held bytes are reserved"
    );
    r.models.clear();
    assert_eq!(r.budget.available(Pool::OpenIndexes), free, "released");
    // a model over cache_bytes is refused
    let small = rig(1 << 20);
    seed(&small);
    // a definition padded with spaces: 300 KB, four times that over the cache
    let big = format!(
        r#"{{"model":{{"name":"big","model":{{"type":"model/linear","definition":"{{\"a\":1,\"b\":2{}}}"}}}}}}"#,
        " ".repeat(300_000)
    );
    assert_eq!(
        call(
            &small,
            "POST",
            "/_ltr/_featureset/fs/_createmodel",
            Some(&big)
        )
        .status,
        201
    );
    assert!(matches!(
        small.models.resolve(None, "big"),
        Err(ResolveError::TooBig(_))
    ));
    assert!(small.models.resolve(None, "m").is_ok());
}

// AC-14
#[test]
fn stats_count_the_caches_and_the_stores_and_a_clear_forgets_a_store() {
    let r = rig(8 << 20);
    seed(&r);
    r.models.resolve(None, "m").expect("resolves");
    r.models.resolve(None, "m").expect("resolves");
    r.models.feature_set(None, "fs").expect("resolves");
    assert!(r.models.resolve(None, "nope").is_err());
    let stats = |target: &str| -> serde_json::Value {
        let reply = call(&r, "GET", target, None);
        assert_eq!(reply.status, 200, "{}", reply.text());
        serde_json::from_str(&reply.text()).expect("json")
    };
    let all = stats("/_ltr/_stats");
    assert_eq!(all["_nodes"]["successful"], 1);
    assert_eq!(all["status"], "green");
    assert_eq!(all["stores"][".ltrstore"]["model_count"], 1);
    assert_eq!(all["stores"][".ltrstore"]["featureset_count"], 1);
    let node = &all["nodes"]["n1"];
    assert_eq!(node["cache"]["model"]["miss_count"], 1);
    assert_eq!(node["cache"]["model"]["hit_count"], 1);
    assert_eq!(node["cache"]["model"]["entry_count"], 1);
    assert_eq!(node["cache"]["featureset"]["miss_count"], 1);
    assert_eq!(node["request_total_count"], 4);
    assert_eq!(node["request_error_count"], 1);
    assert!(node["cache"]["model"]["memory_usage_in_bytes"].as_u64() > Some(0));
    // a named stat answers only that
    let one = stats("/_plugins/_ltr/_stats/request_total_count");
    assert_eq!(one["nodes"]["n1"]["request_total_count"], 4);
    assert!(one.get("stores").is_none() && one["nodes"]["n1"].get("cache").is_none());
    assert_eq!(call(&r, "GET", "/_ltr/_stats/zzz", None).status, 400);
    // a clear forgets the store's entries and returns their bytes
    let held = r.budget.available(Pool::OpenIndexes);
    let cleared = call(&r, "POST", "/_ltr/_clearcache", None);
    assert_eq!(cleared.status, 200, "{}", cleared.text());
    assert_eq!(r.models.held(), (0, 0));
    assert!(r.budget.available(Pool::OpenIndexes) > held);
    assert_eq!(
        call(&r, "POST", "/_ltr/nostore/_clearcache", None).status,
        404
    );
    let before = gets(&r);
    r.models.resolve(None, "m").expect("resolves");
    assert_eq!(gets(&r) - before, 3, "a cleared entry is read again");
}
