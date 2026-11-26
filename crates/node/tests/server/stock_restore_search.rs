//! docs/specs/stock-snapshot-restore AC-4: a snapshot recorded from a stock
//! OpenSearch 3.10 node (`fixtures/snapshots/stock/README.md`) restored over
//! REST, and a reader over the same store answering the searches the stock
//! node answered when it was recorded.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// The recorded repository is a few KiB of fixture, copied to a temporary directory.
#![allow(clippy::disallowed_methods)]

use crate::support;

use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use oss_blob::{BlobStore, FsStore, PooledStore, SourceRoots};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::index_delete::{DeleteService, NoSnapshots};
use oss_node::search::start_reader;
use oss_node::snapshot_restore_route::{SnapshotRestoreService, routes as restore_routes};
use oss_node::snapshot_route::SnapshotService;
use oss_node::stock_restore_route::StockRestoreSettings;
use oss_rest::Router;
use oss_testkit::TempDir;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("directory");
    for entry in std::fs::read_dir(from).expect("reads").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copies");
        }
    }
}

fn hits(body: &str) -> (u64, Vec<String>) {
    let json: serde_json::Value = serde_json::from_str(body).expect("json");
    let total = json["hits"]["total"]["value"].as_u64().expect("total");
    let ids = json["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["_id"].as_str().expect("id").to_owned())
        .collect();
    (total, ids)
}

#[test]
fn a_recorded_stock_snapshot_restores_and_answers_what_the_stock_node_answered() {
    let root = TempDir::new("stock-search");
    let store = Arc::new(FsStore::new(root.path().join("node")).expect("store"));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let sources = root.path().join("sources");
    let repo = sources.join("repo");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock/repo"),
        &repo,
    );
    let sources = sources.canonicalize().expect("sources");
    let repo = sources.join("repo");

    let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
    let descriptors =
        || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
    let register = Arc::new(SnapshotService::new(
        Arc::clone(&blobs),
        "",
        "node-1",
        Arc::clone(&clock),
    ));
    let deletes = Arc::new(
        DeleteService::new(
            &blobs,
            "",
            DeleteMode::Conditional,
            Arc::clone(&clock),
            Arc::new(NoSnapshots),
            1 << 16,
            8,
        )
        .unwrap(),
    );
    let restorer = Arc::new(
        SnapshotRestoreService::new(
            Arc::clone(&blobs),
            "",
            descriptors(),
            deletes,
            clock,
            true,
            1 << 16,
        )
        .with_stock(StockRestoreSettings {
            enabled: true,
            roots: SourceRoots::parse(sources.to_str().unwrap(), ""),
            fanout: 1,
            s3: None,
        }),
    );
    let router = oss_node::snapshot_route::routes(Router::new(), &register, None).expect("routes");
    let router = restore_routes(router, restorer).expect("restore routes");
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 200 << 20)
        .expect("fits");
    let admin = oss_rest::Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&config),
    )
    .expect("starts");
    let call = |method: &str, target: &str, body: Option<&str>| {
        support::request(admin.local_addr(), method, target, body.map(str::as_bytes))
    };
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        repo.display()
    );
    assert_eq!(call("PUT", "/_snapshot/stock", Some(&body)).status, 200);
    let restored = call(
        "POST",
        "/_snapshot/stock/snap-1/_restore?wait_for_completion=true",
        Some(r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#),
    );
    assert_eq!(restored.status, 200, "{}", restored.text());

    let reader_config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&reader_config),
        Pool::Transport,
        SMALL_BUFFER,
        16,
    )
    .expect("fits");
    let reading = Arc::new(PooledStore::new(
        FsStore::new(root.path().join("node")).expect("store"),
        buffers,
    ));
    let (reader, _service) = start_reader(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        reading,
        "",
        "reader-1",
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let search = |query: &str| {
        let found = support::request(
            reader.local_addr(),
            "POST",
            "/copy/_search",
            Some(query.as_bytes()),
        );
        assert_eq!(found.status, 200, "{}", found.text());
        hits(&found.text())
    };
    // The answers the stock 3.10 node gave when the snapshot was recorded.
    assert_eq!(search(r#"{"size":0}"#).0, 20);
    let (total, ids) =
        search(r#"{"query":{"match":{"title":"gamma"}},"size":10,"sort":[{"n":"asc"}]}"#);
    assert_eq!(
        (total, ids),
        (
            4,
            vec!["2", "7", "12", "17"]
                .into_iter()
                .map(String::from)
                .collect()
        )
    );
    assert_eq!(
        search(r#"{"size":0,"query":{"term":{"level":"warn"}}}"#).0,
        10
    );
}
