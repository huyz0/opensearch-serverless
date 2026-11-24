//! docs/specs/usage-top: the usage report narrowed to the costliest.

#![cfg(all(feature = "reader", feature = "writer"))]

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_blob::guard::Mode;
use oss_blob::usage::Ledger;
use oss_mem::MemoryBudget;
use oss_node::tenants::TenantLevels;
use oss_node::usage_route::{UsageService, routes};
use oss_rest::{Router, Server};

struct Logs;

impl oss_blob::usage::Classifier for Logs {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        key.strip_prefix("i/")?.split('/').next()
    }
}

/// A server over a ledger where `a` cost 1 put, `b` 3, `c` 2, `d` 4.
fn serve() -> (Server, Arc<TenantLevels>) {
    let ledger = Arc::new(Ledger::new(16));
    let store = oss_blob::UsageStore::new(
        oss_blob::Emulator::new(1 << 20, 1),
        Arc::clone(&ledger),
        Arc::new(Logs),
    );
    let levels = Arc::new(TenantLevels::new(Arc::clone(&ledger), Mode::Observe, 1_000));
    levels.tick(0);
    // A tenant is first seen by the tick after its first work, so its rate
    // is read at the second.
    for at in [10_000, 20_000] {
        for (name, puts) in [("a", 1), ("b", 3), ("c", 2), ("d", 4)] {
            for i in 0..puts {
                store
                    .put(
                        &format!("i/{name}/{i}"),
                        &mut oss_blob::SliceSource::new(b"x"),
                        &mut [0u8; 16],
                    )
                    .expect("put");
            }
        }
        levels.tick(at);
    }
    let service =
        UsageService::new(ledger, "n1", 5, Arc::new(|| 20_000)).with_tenants(Arc::clone(&levels));
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    (server, levels)
}

fn get(server: &Server, target: &str) -> support::Reply {
    support::request(server.local_addr(), "GET", target, None)
}

// AC-1
#[test]
fn top_lists_the_costliest_indices_highest_first_with_the_node_lines() {
    let (server, _levels) = serve();
    let reply = get(&server, "/_serverless/usage?top=2");
    assert_eq!(reply.status, 200, "{}", reply.text());
    let body = reply.text();
    let indices = &body[..body.find("\"tenant_levels\"").expect("tenants")];
    let d = indices.find("\"d\":{").expect("d");
    let b = indices.find("\"b\":{").expect("b");
    assert!(d < b, "{body}");
    assert!(
        !indices.contains("\"a\":{") && !indices.contains("\"c\":{"),
        "{body}"
    );
    assert!(
        indices.contains("\"_node\":{") && indices.contains("\"_other\":{"),
        "{body}"
    );
}

// AC-2
#[test]
fn top_lists_the_tenants_with_the_highest_cost_a_minute() {
    let (server, _levels) = serve();
    let body = get(&server, "/_serverless/usage?top=2").text();
    let tenants = &body[body.find("\"tenant_levels\"").expect("tenants")..];
    assert!(
        tenants.contains("\"d\":{\"level\"") && tenants.contains("\"b\":{\"level\""),
        "{body}"
    );
    assert!(
        !tenants.contains("\"a\":{\"level\"") && !tenants.contains("\"c\":{\"level\""),
        "{body}"
    );
}

// AC-3
#[test]
fn without_top_every_index_and_tenant_is_listed() {
    let (server, _levels) = serve();
    let body = get(&server, "/_serverless/usage").text();
    for name in ["a", "b", "c", "d"] {
        assert!(
            body.contains(&format!("\"{name}\":{{\"cost_units\"")),
            "{body}"
        );
        assert!(body.contains(&format!("\"{name}\":{{\"level\"")), "{body}");
    }
}

// AC-4
#[test]
fn a_top_that_is_not_a_positive_whole_number_is_refused() {
    let (server, _levels) = serve();
    for bad in ["0", "-1", "x", ""] {
        let reply = get(&server, &format!("/_serverless/usage?top={bad}"));
        assert_eq!(reply.status, 400, "top={bad}: {}", reply.text());
    }
}
