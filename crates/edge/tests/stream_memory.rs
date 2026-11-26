// ARITH: byte counts in a test; an overflow panics the test, which fails it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]
//! A result several times the message limit streams through `ServerStreamSearch` with the heap
//! flat. Its own binary: the counting allocator's peak is a process maximum, so nothing larger may
//! run before it.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use support::{State, WORKERS, h2c, start_with};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

/// Heap above the baseline the stream may add: one page in the front and the client's frames.
const ALLOWANCE: usize = 2 << 20;

/// The wire form of a `ServerStreamSearch` request for index `a`: pages of `size` sorted by `n`.
fn request(size: u32) -> Bytes {
    use prost::Message;
    let json = serde_json::json!({"index": ["a"], "searchRequestBody": {"size": size, "sort": [{"field": "n"}]}});
    let message =
        prost_reflect::DynamicMessage::deserialize(oss_edge::proto_message("SearchRequest"), json)
            .expect("protojson")
            .encode_to_vec();
    support::grpc_frame(0, &message)
}

/// Reads a stream without keeping it, optionally pausing between frames: its bytes and frames.
async fn read_stream(addr: std::net::SocketAddr, pause: Option<Duration>) -> (usize, usize) {
    let mut send = h2c(addr).await;
    let call = http::Request::builder()
        .method("POST")
        .uri("http://node/org.opensearch.protobufs.services.SearchService/ServerStreamSearch")
        .header("content-type", "application/grpc")
        .body(())
        .expect("request");
    let (response, mut stream) = send.send_request(call, false).expect("sent");
    stream.send_data(request(40), true).expect("body");
    let mut body = response.await.expect("response").into_body();
    let (mut bytes, mut frames) = (0usize, 0usize);
    while let Some(data) = body.data().await {
        let data = data.expect("data");
        bytes += data.len();
        frames += 1;
        body.flow_control()
            .release_capacity(data.len())
            .expect("credit");
        if let Some(pause) = pause {
            tokio::time::sleep(pause).await;
        }
    }
    let trailers = body.trailers().await.expect("trailers").expect("a status");
    assert_eq!(
        trailers.get("grpc-status").map(|v| v.as_bytes()),
        Some(&b"0"[..])
    );
    (bytes, frames)
}

// AC-11
#[tokio::test(flavor = "multi_thread")]
async fn a_result_larger_than_the_message_limit_streams_without_growing_the_heap() {
    let state = Arc::new(State::default());
    state.corpus.store(800, Ordering::Release);
    state.padding.store(1000, Ordering::Release);
    let (server, budget, upstream) = start_with(&state, 64 << 20);
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.grpc = Some("127.0.0.1:0".parse().expect("addr"));
    c.grpc_max_message = 64 << 10;
    let limit = c.grpc_max_message;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.grpc_addr().expect("bound");
    let _ = read_stream(addr, None).await;
    for pause in [None, Some(Duration::from_millis(20))] {
        let window = PeakWindow::start(&ALLOC);
        let (bytes, frames) = read_stream(addr, pause).await;
        let peak = window.peak_bytes();
        println!(
            "stream of {bytes} bytes in {frames} frames, limit {limit}: peak {peak} bytes above the start (pause {pause:?})"
        );
        assert!(
            bytes > 4 * limit,
            "the result ({bytes}) is more than four messages of {limit}"
        );
        assert!(
            peak <= ALLOWANCE,
            "heap above the baseline ({peak}) must not grow with the result"
        );
    }
    // Four times the result moves the process's high-water mark by no more than a page's worth.
    state.corpus.store(3200, Ordering::Release);
    let before = ALLOC.max_allocated();
    let (bytes, _) = read_stream(addr, None).await;
    let rise = ALLOC.max_allocated().saturating_sub(before);
    println!("stream of {bytes} bytes raised the high-water mark by {rise} bytes");
    assert!(bytes > 16 * limit);
    assert!(
        rise <= 256 << 10,
        "the high-water mark rose {rise} bytes for a result four times larger"
    );
    edge.shutdown();
    server.shutdown();
}
