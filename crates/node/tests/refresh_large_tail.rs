//! docs/specs/refresh-large-tail: a refresh makes a large unpublished tail
//! searchable at once. Its own binary: it sends a few megabytes a document.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use oss_node::boot::{NodeConfig, start};
use oss_testkit::TempDir;

const DOCS: usize = 5;
const DOC_BYTES: usize = 3 << 20;

const ROUNDS: usize = 1;

fn bulk_body(round: usize) -> Vec<u8> {
    let mut body = Vec::new();
    for i in 0..DOCS {
        body.extend_from_slice(
            format!("{{\"index\":{{\"_index\":\"big\",\"_id\":\"r{round}d{i}\"}}}}\n").as_bytes(),
        );
        body.extend_from_slice(br#"{"text":""#);
        let start = body.len();
        while body.len() - start < DOC_BYTES {
            body.extend_from_slice(b"alpha beta gamma delta ");
        }
        body.truncate(start + DOC_BYTES);
        body.extend_from_slice(b"\"}\n");
    }
    body
}

/// AC-1
#[test]
fn the_first_count_after_a_refresh_sees_every_acknowledged_large_document() {
    let dir = TempDir::new("refresh-large-tail");
    let root = dir.path().display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        _ => None,
    };
    let node = start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts");
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/big", None).status, 200);
    // Each round leaves a fresh unpublished tail of DOCS large documents.
    for round in 1..=ROUNDS {
        let reply = support::request(addr, "POST", "/_bulk", Some(&bulk_body(round)));
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert_eq!(
            support::request(addr, "POST", "/big/_refresh", None).status,
            200
        );
        let count = support::request(addr, "GET", "/big/_count", None).text();
        assert!(
            count.contains(&format!("\"count\":{}", DOCS * round)),
            "round {round}: {count}"
        );
    }
}
