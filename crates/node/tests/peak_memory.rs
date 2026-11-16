//! Peak memory under concurrent searches: eight clients at once, half
//! sending an 8 MiB body (a term query whose value alone is larger than a
//! search body may record) and half asking for the whole result window, to
//! a reader node. The process's heap peak stays within the recordings'
//! limit, plus a fixed allowance, under half the bytes sent: a body is
//! never held whole, and a recording stops at its limit. Searches reserve
//! from the request-body pool before they grow, and the peak stays within
//! it. Its own binary, because the counting allocator is
//! process-wide.

// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::thread;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_reader::dsl::MAX_RECORDED_BYTES;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

use support::{request, start_with, store};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const CLIENTS: usize = 8;
const BODY: usize = 8 << 20;
/// The request-body pool: the server's own buffers, then what searches
/// reserve. Large enough that no search is refused for want of it, so each
/// client's answer is known.
const POOL: usize = 32 << 20;
/// Heap besides the recordings: client threads and their replies, the
/// windows' hits, the analyzer and engine state of each search. The whole
/// peak was measured at 3.3 to 4.4 MiB in a debug build.
const ALLOWANCE: usize = 2 << 20;

fn large_body() -> Vec<u8> {
    let mut b = Vec::with_capacity(BODY + 64);
    b.extend_from_slice(br#"{"query":{"term":{"tag":""#);
    b.resize(BODY, b'a');
    b.extend_from_slice(br#""}}}"#);
    b
}

#[test]
fn concurrent_large_bodies_and_windows_stay_within_the_pool() {
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, POOL)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 8 << 20))
            .expect("fits"),
    );
    let node = start_with(store(), &budget);
    let addr = node.addr();
    // Open the index and warm its pages before measuring.
    let warm = request(
        addr,
        "POST",
        "/compat-paging-sort/_search",
        Some(br#"{"size":10000}"#),
    );
    assert_eq!(warm.status, 200, "{}", warm.text());
    // Built before the window opens, and never copied.
    let body = std::sync::Arc::new(large_body());
    let window = PeakWindow::start(&ALLOC);
    let clients: Vec<_> = (0..CLIENTS)
        .map(|i| {
            let body = std::sync::Arc::clone(&body);
            thread::spawn(move || {
                if i % 2 == 0 {
                    request(addr, "POST", "/compat-paging-sort/_search", Some(&body)).status
                } else {
                    request(
                        addr,
                        "POST",
                        "/compat-paging-sort/_search",
                        Some(br#"{"size":10000,"sort":[{"tag":"asc"},{"year":"desc"}]}"#),
                    )
                    .status
                }
            })
        })
        .collect();
    let statuses: Vec<u16> = clients
        .into_iter()
        .map(|c| c.join().expect("client"))
        .collect();
    let peak = window.peak_bytes();
    node.server.shutdown();
    println!("peak {peak} bytes; pool {POOL}, statuses {statuses:?}");
    for (i, status) in statuses.iter().enumerate() {
        let want = if i % 2 == 0 { 400 } else { 200 };
        assert_eq!(*status, want, "client {i}: {statuses:?}");
    }
    // Each large body's recording stops at its limit, and a growing buffer
    // may double past what it holds; the windows' hits fit the allowance.
    let bound = CLIENTS / 2 * 2 * MAX_RECORDED_BYTES + ALLOWANCE;
    assert!(peak <= bound, "peak {peak} over {bound}");
    assert!(
        peak <= POOL,
        "peak {peak} over the pool the searches reserved from"
    );
    assert!(
        bound < CLIENTS / 2 * BODY / 2,
        "the bound is under half the bytes the large bodies sent"
    );
}
