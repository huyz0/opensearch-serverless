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
    models: StoreModels,
    budget: MemoryBudget,
}

fn rig(cache_bytes: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(LtrService::new(&blobs, "", 1 << 16));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
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
    let models = StoreModels::new(&blobs, "", cache, pool.clone());
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
