//! docs/specs/extension-analyzer AC-18: a reader whose extensions pool cannot
//! hold the module an index pins answers 429 `extension_busy`, not a
//! fallback to the built-in analyzer, and the next search opens once the
//! room is back.
#![cfg(feature = "extensions")]

use std::sync::Arc;

use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore};
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_modules::ModuleSource;
use oss_testkit::TempDir;

use super::pinned_read::{module, rig};
use crate::support::request;

// AC-18 (429 through a running search, a real full pool)
#[test]
fn a_search_whose_pool_cannot_hold_the_module_is_a_429_and_the_next_one_opens() {
    let root = TempDir::new("pinned-full-pool");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let source = ModuleSource::new(
        Arc::new(FsStore::new(root.path()).expect("store")) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget.clone(),
        16,
    );
    assert!(r.service.set_modules(Arc::new(source)));
    // Every byte but one MiB of the pool is held by something else.
    let taken = budget
        .try_reserve(Pool::Extensions, budget.limit(Pool::Extensions) - (1 << 20))
        .expect("reserved");
    let body = br#"{"query":{"match":{"t":"zzzzzzz"}}}"#;
    let busy = request(r.server.local_addr(), "POST", "/logs/_search", Some(body));
    assert_eq!(busy.status, 429, "{}", busy.text());
    assert!(busy.text().contains("extension_busy"), "{}", busy.text());
    assert!(!busy.text().contains(r#""hits""#), "{}", busy.text());
    drop(taken);
    let again = request(r.server.local_addr(), "POST", "/logs/_search", Some(body));
    assert_eq!(again.status, 200, "{}", again.text());
    assert!(again.text().contains(r#""value":1"#), "{}", again.text());
}
