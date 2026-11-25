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

/// AC-2 of docs/specs/refresh-large-tail
#[test]
fn the_first_count_after_a_refresh_sees_every_acknowledged_large_document() {
    rounds(1, "1");
}

/// AC-1 of docs/specs/refresh-count-after-publish: a count right after a
/// refresh is not refused when a publish landed since the shard was opened.
#[test]
fn a_count_after_a_refresh_is_answered_when_a_publish_landed_since_the_shard_opened() {
    rounds(3, "1");
}

fn rounds(rounds: usize, depth: &str) {
    let dir = TempDir::new("refresh-large-tail");
    let root = dir.path().display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_WRITE_PIPELINE_DEPTH" => Some(depth.to_owned()),
        _ => None,
    };
    let node = start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts");
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/big", None).status, 200);
    // Each round leaves a fresh unpublished tail of DOCS large documents.
    for round in 1..=rounds {
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

/// AC-3 of docs/specs/refresh-large-tail: the same count when the shard's
/// groups wait for the log together, so the first lazy publish starts late
/// and the reader finds the shard owned with no commit yet.
#[test]
fn the_first_count_after_a_refresh_sees_every_large_document_with_a_pipeline() {
    rounds(1, "8");
}

/// AC-3 of docs/specs/refresh-large-tail: a register lost from a shard that
/// has published is answered with every document its owner holds, or
/// refused, never with fewer, though its owner is live.
#[test]
fn a_lost_register_of_a_published_shard_never_answers_with_fewer_documents() {
    let dir = TempDir::new("refresh-lost-register");
    let root = dir.path().display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_WRITE_PIPELINE_DEPTH" => Some("1".to_owned()),
        _ => None,
    };
    let node = start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts");
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/lost", None).status, 200);
    let body = b"{\"index\":{\"_index\":\"lost\",\"_id\":\"1\"}}\n{\"a\":\"b\"}\n";
    assert_eq!(
        support::request(addr, "POST", "/_bulk", Some(body)).status,
        200
    );
    assert_eq!(
        support::request(addr, "POST", "/lost/_refresh", None).status,
        200
    );
    let mut removed = 0;
    let mut stack = vec![dir.path().to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("dir").flatten() {
            let path = entry.path();
            let name = path.display().to_string();
            if path.is_dir() {
                stack.push(path);
            } else if name.contains("/lost#") && name.ends_with("/manifest") {
                std::fs::remove_file(&path).expect("removes");
                removed += 1;
            }
        }
    }
    assert!(removed > 0, "the commit register was found");
    let reply = support::request(addr, "GET", "/lost/_count", None);
    assert!(
        reply.status != 200 || reply.text().contains("\"count\":1"),
        "{}",
        reply.text()
    );
}
