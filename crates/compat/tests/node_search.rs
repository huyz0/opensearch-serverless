//! Replays the search fixtures (fixtures/compat/search) against a reader
//! node over HTTP. The node cannot index, so each fixture's index comes
//! from the blobs the Java shell wrote for the same mapping and documents
//! (fixtures/compat-blobs), loaded into the in-process emulator; the steps
//! that create and fill an index, and the cleanup, are not sent. Every
//! `_search` step must match its recording after the fixture's own
//! normalisation.

// ARITH: test offsets and sizes are small; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_compat::{Fixture, StepOutcome, load_dir, run_suite};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_rest::ServerConfig;

mod support;
use support::HttpTarget;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("blob dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(base, &path, out);
        } else if path.file_name().is_some_and(|n| n != "README.md") {
            let key = path
                .strip_prefix(base)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((key, std::fs::read(&path).expect("read blob")));
        }
    }
}

fn store() -> Arc<PooledStore<Emulator>> {
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 16).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let base = root().join("compat-blobs");
    let mut blobs = Vec::new();
    walk(&base, &base, &mut blobs);
    for (key, bytes) in &blobs {
        store
            .put(key, &mut SliceSource::new(bytes))
            .expect("load blob");
    }
    Arc::new(store)
}

/// The fixtures' `_search` steps alone, without their cleanup.
fn search_steps(fixtures: Vec<Fixture>) -> Vec<Fixture> {
    fixtures
        .into_iter()
        .map(|mut f| {
            f.cleanup.clear();
            f.steps.retain(|s| s.request.path.ends_with("/_search"));
            support::normalise_columns(&mut f);
            f
        })
        .collect()
}

#[test]
fn the_reader_node_answers_every_search_fixture_as_opensearch_did() {
    let fixtures: Vec<Fixture> = load_dir(&root().join("compat/search"))
        .expect("fixtures load")
        .into_iter()
        .map(|(_, f)| f)
        .collect();
    let fixtures = search_steps(fixtures);
    let steps: usize = fixtures.iter().map(|f| f.steps.len()).sum();
    assert!(steps >= 50, "the search fixtures hold {steps} searches");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, 16 << 20)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let config = ServerConfig {
        workers: 2,
        max_connections: 8,
        body_buffers: 2,
        ..ServerConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let (server, _) =
        oss_node::search::start_reader(listener, config, store(), "", "node-1", &budget)
            .expect("starts");
    let mut target = HttpTarget {
        addr: server.local_addr(),
    };
    let report = run_suite(&mut target, &fixtures);
    let failed: Vec<String> = report
        .fixtures
        .iter()
        .flat_map(|f| {
            f.steps
                .iter()
                .filter(|s| s.outcome != StepOutcome::Passed)
                .map(move |s| format!("{} / {}: {:?}", f.name, s.name, s.outcome))
        })
        .collect();
    server.shutdown();
    assert!(
        failed.is_empty(),
        "{} of {steps} searches differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
