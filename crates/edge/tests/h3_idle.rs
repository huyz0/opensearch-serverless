#![allow(missing_docs)]
//! What an idle HTTP/3 connection costs. Its own binary: the counting allocator is process-wide.

mod support;

use std::sync::Arc;

use oss_edge::{DEFAULT_H3_OVERHEAD, Edge, EdgeConfig, H3_ENDPOINT_BYTES};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use support::quic;
use support::{State, WORKERS, start_with};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const CONNECTIONS: usize = 400;

#[tokio::test(flavor = "multi_thread")]
async fn idle_connections_cost_less_than_the_overhead_charged_for_each() {
    let state = Arc::new(State::default());
    let (server, _unused, upstream) = start_with(&state, 1 << 20);
    let budget = support::budget(2 << 30);
    let p = quic::pair("idle-memory");
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h3 = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = Some(p.cert.clone());
    c.key = Some(p.key.clone());
    c.max_connections = CONNECTIONS;
    // The server's half of a connection is charged its own overhead and the QUIC one.
    let charged = c.h3_connection_overhead;
    let started = PeakWindow::start(&ALLOC);
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let listener = started.live_bytes();
    println!(
        "H3IDLE|the front with an h3 listener and no connection holds {listener} bytes; charged {H3_ENDPOINT_BYTES}"
    );
    assert!(
        listener <= H3_ENDPOINT_BYTES,
        "the listener holds {listener} bytes, over the {H3_ENDPOINT_BYTES} charged for it"
    );
    // One client socket for all of them, so the client halves are connection state, not sockets.
    let endpoint = quic::endpoint(&p, &[b"h3"]);
    let window = PeakWindow::start(&ALLOC);
    let baseline = window.live_bytes();
    let mut held = Vec::new();
    for i in 0..CONNECTIONS {
        let (conn, mut send) = quic::connect(&endpoint, addr).await;
        if i % 50 == 0 {
            assert_eq!(
                quic::request(&mut send, "GET", "https://x/", None)
                    .await
                    .expect("answer")
                    .status,
                200
            );
        }
        held.push((conn, send));
    }
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let live = window.live_bytes().saturating_sub(baseline);
    let both = live / CONNECTIONS;
    println!(
        "H3IDLE|{CONNECTIONS} idle connections: {live} bytes live, {both} per connection for both ends, about {} for the server's half; charged {charged} (default h3 overhead {DEFAULT_H3_OVERHEAD}); provisional: debug build, loopback",
        both / 2
    );
    drop(held);
    edge.shutdown();
    server.shutdown();
    assert!(
        both / 2 <= charged,
        "half of {both} bytes per connection is over the {charged} charged for one"
    );
}
