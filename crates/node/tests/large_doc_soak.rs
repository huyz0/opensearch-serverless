//! docs/specs/large-doc-soak: a bulk of about 100 MB of large documents
//! to a node at its smallest memory. Its own binary, because the counting
//! allocator is process-wide.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use oss_node::boot::{NodeConfig, start};
use oss_testkit::{CountingAllocator, PeakWindow, TempDir, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const DOCS: usize = 24;
const DOC_BYTES: usize = 4 << 20;
/// The memory the node starts with, and the bound its heap peak keeps.
const BUDGET: usize = 512 << 20;

/// An ndjson bulk of `DOCS` documents, each a text field of `DOC_BYTES`.
fn bulk_body() -> Vec<u8> {
    let mut body = Vec::with_capacity(DOCS * (DOC_BYTES + 128));
    for i in 0..DOCS {
        body.extend_from_slice(
            format!("{{\"index\":{{\"_index\":\"big\",\"_id\":\"d{i}\"}}}}\n").as_bytes(),
        );
        body.extend_from_slice(br#"{"text":""#);
        // Words, so the analyzer sees terms rather than one huge token.
        let start = body.len();
        while body.len() - start < DOC_BYTES {
            body.extend_from_slice(b"alpha beta gamma delta ");
        }
        body.truncate(start + DOC_BYTES);
        body.extend_from_slice(b"\"}\n");
    }
    body
}

#[test]
fn a_bulk_of_a_hundred_megabytes_of_large_documents_stays_within_the_budget() {
    let dir = TempDir::new("large-doc-soak");
    let root = dir.path().display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_MEMORY_BYTES" => Some(BUDGET.to_string()),
        _ => None,
    };
    let node = start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts");
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/big", None).status, 200);
    // Built before the window opens, and never copied.
    let body = bulk_body();
    let sent = body.len();
    let window = PeakWindow::start(&ALLOC);
    let reply = support::request(addr, "POST", "/_bulk", Some(&body));
    let peak = window.peak_bytes();
    println!("peak {peak} bytes for {sent} sent; status {}", reply.status);
    // AC-1
    assert!(
        reply.status == 200 || reply.status == 429,
        "{}",
        &reply.text()[..reply.text().len().min(500)]
    );
    if reply.status == 200 {
        let text = reply.text();
        assert!(
            text.contains("\"errors\":false"),
            "{}",
            &text[..text.len().min(500)]
        );
        // AC-2: a refresh, then the count, polled; the first count after a
        // refresh was seen short while the readers caught up.
        let started = std::time::Instant::now();
        let mut first = None;
        loop {
            assert_eq!(
                support::request(addr, "POST", "/big/_refresh", None).status,
                200
            );
            let count = support::request(addr, "GET", "/big/_count", None).text();
            first.get_or_insert_with(|| count.clone());
            if count.contains(&format!("\"count\":{DOCS}")) {
                break;
            }
            assert!(
                started.elapsed().as_secs() < 30,
                "never saw {DOCS}: {count}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!(
            "first count {:?}, all seen after {:?}",
            first,
            started.elapsed()
        );
    }
    // AC-3
    assert!(peak < BUDGET, "peak {peak} over the budget {BUDGET}");
    assert!(
        peak < sent + (64 << 20),
        "peak {peak} over the bytes sent {sent} plus 64 MiB"
    );
}
