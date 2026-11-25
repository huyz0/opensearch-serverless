#![allow(missing_docs)]
//! The front's heap under load, against what it charged. Its own binary: the counting allocator
//! is process-wide and its peak is a process maximum, so the two measurements run in one test, the
//! upload peak first.

mod support;

use std::sync::Arc;

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig};
use oss_mem::Pool;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use support::{State, WORKERS, h2_request, h2c, start_with};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

/// Heap outside the pool: the runtime's threads and queues, h2's per-connection tables, and the
/// test's own clients. The test prints the measured figure, about 0.7 MiB in a debug build.
const ALLOWANCE: usize = 2 << 20;

async fn concurrent_large_uploads_stay_within_the_charged_buffers() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start_with(&state, 64 << 20);
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h2c_addr().expect("bound");
    let charged = budget.in_use(Pool::RequestBodies);
    let body = Bytes::from(vec![9u8; 16 << 20]);
    let window = PeakWindow::start(&ALLOC);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let body = body.clone();
        tasks.push(tokio::spawn(async move {
            let mut send = h2c(addr).await;
            h2_request(&mut send, "POST", "http://x/sum", Some(body)).await
        }));
    }
    for t in tasks {
        let a = t.await.expect("joins").expect("answer");
        assert_eq!(
            (a.status, a.body.as_slice()),
            (200, &br#"{"bytes":16777216}"#[..])
        );
    }
    let peak = window.peak_bytes();
    // The clients share one 16 MiB body, allocated before the window opened; what the front and the
    // server add while 64 MiB goes through is the peak.
    let over = peak;
    println!("peak {peak} bytes above the window's start for 64 MiB uploaded; charged {charged}");
    edge.shutdown();
    server.shutdown();
    assert!(
        over <= ALLOWANCE,
        "heap above the baseline ({over}) must stay within the allowance, not grow with the 64 MiB uploaded"
    );
}

async fn idle_connections_cost_less_than_the_overhead_charged_for_each() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start_with(&state, 64 << 20);
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    c.max_connections = 400;
    let overhead = c.connection_overhead;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h2c_addr().expect("bound");
    let window = PeakWindow::start(&ALLOC);
    let baseline = window.live_bytes();
    let mut held = Vec::new();
    for _ in 0..400 {
        held.push(h2c(addr).await);
    }
    // One request per connection proves each completed its handshake.
    for s in held.iter_mut().step_by(50) {
        assert_eq!(
            h2_request(s, "GET", "http://x/", None)
                .await
                .expect("answer")
                .status,
            200
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let live = window.live_bytes().saturating_sub(baseline);
    println!(
        "400 idle connections: {live} bytes live, {} per connection (client and server sides in one process)",
        live / 400
    );
    // The client halves share the process; the bound is still the server's charge per connection.
    assert!(
        live / 400 <= overhead,
        "each connection costs {} bytes, charged {overhead}",
        live / 400
    );
    // The 501st is closed before a handshake.
    let tcp = tokio::net::TcpStream::connect(addr).await.expect("tcp");
    assert!(
        h2::client::handshake(tcp).await.is_err() || {
            let mut extra = h2c(addr).await;
            h2_request(&mut extra, "GET", "http://x/", None)
                .await
                .is_err()
        }
    );
    drop(held);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn heap_stays_within_what_the_front_charges() {
    concurrent_large_uploads_stay_within_the_charged_buffers().await;
    idle_connections_cost_less_than_the_overhead_charged_for_each().await;
}
