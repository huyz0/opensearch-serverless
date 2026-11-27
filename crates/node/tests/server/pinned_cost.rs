//! docs/specs/extension-analyzer AC-19: how many module calls a query makes,
//! read from the host's meter through a running reader, and the time of a
//! search on a pinned index against the same count of calls made by hand.

use super::pinned_read::{TEXT_QUERIES, matched, module, rig};
use crate::support::request;

use std::sync::Arc;
use std::time::Instant;

use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore};
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_modules::ModuleSource;
use oss_testkit::TempDir;

fn calls(host: &Host) -> u64 {
    host.meter().snapshot().iter().map(|(_, u)| u.calls).sum()
}

// AC-19: a single-term text query calls the module once
#[test]
fn a_text_query_on_a_pinned_index_makes_one_module_call_a_term() {
    let root = TempDir::new("pinned-cost");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let host = Host::new(HostConfig::default());
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let source = Arc::new(ModuleSource::new(
        Arc::new(FsStore::new(root.path()).expect("store")) as Arc<dyn BlobStore>,
        "",
        host.clone(),
        budget,
        16,
    ));
    assert!(r.service.set_modules(source));
    // The first search opens the index and the module; the probe at load is a call too.
    assert_eq!(matched(&r).status, 200);
    let mut report = Vec::new();
    for (name, query) in TEXT_QUERIES {
        let body = format!(r#"{{"query":{query}}}"#);
        let before = calls(&host);
        let reply = request(
            r.server.local_addr(),
            "POST",
            "/logs/_search",
            Some(body.as_bytes()),
        );
        assert_eq!(reply.status, 200, "{name}: {}", reply.text());
        let made = calls(&host) - before;
        report.push(format!("{name}: {made}"));
        assert_eq!(made, 1, "{name} made {made} module calls");
    }
    // Time of a search, after warm-up, one thread.
    let body = br#"{"query":{"match":{"t":"zzzzzzz"}}}"#;
    for _ in 0..50 {
        request(r.server.local_addr(), "POST", "/logs/_search", Some(body));
    }
    let mut micros: Vec<u128> = (0..500)
        .map(|_| {
            let at = Instant::now();
            request(r.server.local_addr(), "POST", "/logs/_search", Some(body));
            at.elapsed().as_micros()
        })
        .collect();
    micros.sort_unstable();
    println!(
        "calls per query ({}); 500 searches: p50 {} us, p99 {} us",
        report.join(", "),
        micros[250],
        micros[494]
    );
}
