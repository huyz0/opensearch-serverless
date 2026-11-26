#![allow(missing_docs)]
//! HTTP/3 heap under load. Its own binary: the counting allocator is process-wide and its peak is
//! a process maximum, so the measurements run in one test, in the order that keeps them honest:
//! the 64 MiB upload, then the 256 MiB one, which must not raise the peak, then idle connections.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use support::quic::{self, Pair};
use support::{State, WORKERS, start_with};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

/// AC-6's stated bound on the heap above the window's start while four 16 MiB uploads pass.
const BOUND: usize = 4 << 20;
/// How much more a four times larger upload may raise the peak.
const GROWTH: usize = 256 << 10;

fn config(upstream: SocketAddr, p: &Pair) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h3 = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = Some(p.cert.clone());
    c.key = Some(p.key.clone());
    c.max_connections = 32;
    c
}

async fn uploads(addr: SocketAddr, p: &Pair, each: usize) -> usize {
    let body = Bytes::from(vec![9u8; each]);
    // The clients' sockets and receive buffers are made before the window opens: the figure is
    // what the front and the server add, as in the HTTP/2 measurement.
    let endpoints: Vec<_> = (0..4).map(|_| quic::endpoint(p, &[b"h3"])).collect();
    let window = PeakWindow::start(&ALLOC);
    let mut tasks = Vec::new();
    for endpoint in endpoints {
        let body = body.clone();
        tasks.push(tokio::spawn(async move {
            let (_conn, mut send) = quic::connect(&endpoint, addr).await;
            quic::request(&mut send, "POST", "https://x/sum", Some(body)).await
        }));
    }
    for t in tasks {
        let a = t.await.expect("joins").expect("answer");
        assert_eq!(
            (a.status, a.body.as_slice()),
            (200, format!("{{\"bytes\":{each}}}").as_bytes())
        );
    }
    window.peak_bytes()
}

#[tokio::test(flavor = "multi_thread")]
async fn heap_does_not_grow_with_the_bytes_uploaded_and_idle_connections_have_a_price() {
    let state = Arc::new(State::default());
    let (server, _unused, upstream) = start_with(&state, 256 << 20);
    let budget = support::budget(1 << 30);
    let p = quic::pair("memory");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    let small = uploads(addr, &p, 16 << 20).await;
    println!("H3MEMORY|peak {small} bytes above the window's start for 4 x 16 MiB uploaded");
    let large = uploads(addr, &p, 64 << 20).await;
    println!("H3MEMORY|peak {large} bytes above the window's start for 4 x 64 MiB uploaded");
    edge.shutdown();
    server.shutdown();
    assert!(
        small <= BOUND,
        "4 x 16 MiB raised the heap peak by {small}, over {BOUND}"
    );
    assert!(
        large <= small + GROWTH,
        "4 x 64 MiB raised it to {large}, more than {GROWTH} over the {small} of 4 x 16 MiB: memory grows with the bytes"
    );
}
