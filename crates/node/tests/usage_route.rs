//! docs/specs/store-usage-wiring: the usage route's body.

#![cfg(all(feature = "reader", feature = "writer"))]

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::usage::Ledger;
use oss_blob::{BlobStore, S3Op};
use oss_mem::MemoryBudget;
use oss_node::usage_route::{UsageService, routes};
use oss_rest::{Router, Server};

// AC-4
#[test]
fn the_route_lists_every_kind_and_writes_thousandths_as_decimals() {
    let ledger = Arc::new(Ledger::new(4));
    let store = oss_blob::UsageStore::new(
        oss_blob::Emulator::new(1 << 20, 1),
        Arc::clone(&ledger),
        Arc::new(Logs),
    );
    store
        .put(
            "i/logs/a",
            &mut oss_blob::SliceSource::new(b"xy"),
            &mut [0u8; 16],
        )
        .expect("put");
    assert_eq!(ledger.of("logs").requests(S3Op::Put), 1_000);
    // A shared upload: a third of a request to one index, two thirds to the other.
    ledger.split(&[("a", 1), ("b", 2)], || {
        store
            .put(
                "shared",
                &mut oss_blob::SliceSource::new(b""),
                &mut [0u8; 16],
            )
            .expect("put");
    });
    let service = UsageService::new(ledger, "n1", 5, Arc::new(|| 9));
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let reply = support::request(server.local_addr(), "GET", "/_serverless/usage", None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    let body = reply.text();
    assert!(
        body.starts_with(
            "{\"node_id\":\"n1\",\"started_millis\":5,\"now_millis\":9,\"metered\":true,\"indices\":{\"a\":"
        ),
        "{body}"
    );
    assert!(
        body.contains(
            "\"requests\":{\"get\":0.000,\"get_range\":0.000,\"head\":0.000,\"put\":1.000,\"copy\":0.000,\"list\":0.000,\"delete\":0.000},\"bytes\":{\"read\":0,\"written\":2}}"
        ),
        "{body}"
    );
    assert!(body.contains("\"a\":{\"cost_units\":0.333,"), "{body}");
    assert!(body.contains("\"b\":{\"cost_units\":0.667,"), "{body}");
    assert!(body.ends_with("\"_other\":{\"cost_units\":0.000,\"requests\":{\"get\":0.000,\"get_range\":0.000,\"head\":0.000,\"put\":0.000,\"copy\":0.000,\"list\":0.000,\"delete\":0.000},\"bytes\":{\"read\":0,\"written\":0}}}}"), "{body}");
}

struct Logs;

impl oss_blob::usage::Classifier for Logs {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        key.strip_prefix("i/")?.split('/').next()
    }
}
