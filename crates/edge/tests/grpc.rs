#![allow(missing_docs)]
//! gRPC calls through the front to a real HTTP/1.1 server: framing, health, limits.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig, EdgeError};
use oss_mem::Pool;
use support::{State, WORKERS, grpc_call, grpc_frame, h2c, start};

const HEALTH: &str = "/grpc.health.v1.Health/Check";
const SERVING: [u8; 2] = [0x08, 1];
const NOT_SERVING: [u8; 2] = [0x08, 2];

fn config(upstream: std::net::SocketAddr) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.grpc = Some("127.0.0.1:0".parse().expect("addr"));
    c.grpc_max_message = 64 * 1024;
    c
}

#[tokio::test(flavor = "multi_thread")]
async fn no_grpc_address_opens_no_port_and_charges_nothing() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let before = budget.in_use(Pool::RequestBodies);
    let mut c = config(upstream);
    c.grpc = None;
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    let plain = c.charge();
    let edge = Edge::start(c, &budget).expect("starts");
    assert!(edge.grpc_addr().is_none());
    assert_eq!(budget.in_use(Pool::RequestBodies) - before, plain);
    let with = config(upstream).charge();
    assert!(with > plain, "a grpc listener is charged its messages");
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn health_check_tracks_the_node() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let ok = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!((ok.http_status, ok.grpc_status), (200, Some(0)));
    assert_eq!(ok.content_type.as_deref(), Some("application/grpc"));
    assert_eq!(ok.messages, vec![SERVING.to_vec()]);
    state.unhealthy.store(true, Ordering::Release);
    let down = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(down.messages, vec![NOT_SERVING.to_vec()]);
    state.unhealthy.store(false, Ordering::Release);
    let named = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b"\x0a\x03nop"))
        .await
        .expect("reply");
    assert_eq!(
        named.grpc_status,
        Some(5),
        "an unknown service is NOT_FOUND"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_methods_and_other_content_types_are_refused() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let other = grpc_call(&mut send, "/nope.Service/Method", &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(other.grpc_status, Some(12));
    assert!(
        other
            .grpc_message
            .expect("message")
            .contains("/nope.Service/Method")
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://node{HEALTH}"))
        .header("content-type", "application/json")
        .body(())
        .expect("request");
    let (response, _stream) = send.send_request(request, true).expect("sent");
    assert_eq!(response.await.expect("response").status(), 415);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn big_and_compressed_messages_are_refused_before_buffering() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let big = grpc_call(
        &mut send,
        HEALTH,
        &[],
        grpc_frame(0, &vec![0u8; 64 * 1024 + 1]),
    )
    .await
    .expect("reply");
    assert_eq!(big.grpc_status, Some(8));
    // Exactly the limit is read, and then decoded: health finds it malformed, which is not a size error.
    let exact = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, &vec![0u8; 64 * 1024]))
        .await
        .expect("reply");
    assert_eq!(exact.grpc_status, Some(3));
    // A request that ends inside its message is not served as if it were whole.
    let mut cut = vec![0u8, 0, 0, 0, 5];
    cut.truncate(5);
    let cut = grpc_call(&mut send, HEALTH, &[], Bytes::from(cut))
        .await
        .expect("reply");
    assert_eq!(cut.grpc_status, Some(3));
    let flagged = grpc_call(&mut send, HEALTH, &[], grpc_frame(1, b""))
        .await
        .expect("reply");
    assert_eq!(flagged.grpc_status, Some(12));
    let encoded = grpc_call(
        &mut send,
        HEALTH,
        &[("grpc-encoding", "gzip")],
        grpc_frame(0, b""),
    )
    .await
    .expect("reply");
    assert_eq!(encoded.grpc_status, Some(12));
    let two = [grpc_frame(0, b""), grpc_frame(0, b"")].concat();
    let two = grpc_call(&mut send, HEALTH, &[], Bytes::from(two))
        .await
        .expect("reply");
    assert_eq!(two.grpc_status, Some(3), "a unary call takes one message");
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deadline_ends_the_call_and_frees_its_slot() {
    let state = Arc::new(State::default());
    state.hold_health.store(true, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let began = Instant::now();
    let late = grpc_call(
        &mut send,
        HEALTH,
        &[("grpc-timeout", "50m")],
        grpc_frame(0, b""),
    )
    .await
    .expect("reply");
    assert_eq!(late.grpc_status, Some(4));
    assert!(began.elapsed() < Duration::from_secs(2));
    state.release.store(true, Ordering::Release);
    let again = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(again.grpc_status, Some(0));
    let bad = grpc_call(
        &mut send,
        HEALTH,
        &[("grpc-timeout", "5x")],
        grpc_frame(0, b""),
    )
    .await
    .expect("reply");
    assert_eq!(bad.grpc_status, Some(3));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_past_max_streams_are_unavailable() {
    let state = Arc::new(State::default());
    state.hold_health.store(true, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.max_streams = 1;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.grpc_addr().expect("bound");
    let mut first = h2c(addr).await;
    let held =
        tokio::spawn(async move { grpc_call(&mut first, HEALTH, &[], grpc_frame(0, b"")).await });
    // The call is in flight once its handler waits; give it time to get there.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut second = h2c(addr).await;
    let refused = grpc_call(&mut second, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(refused.grpc_status, Some(14));
    state.release.store(true, Ordering::Release);
    assert_eq!(
        held.await.expect("joined").expect("reply").grpc_status,
        Some(0)
    );
    edge.shutdown();
    server.shutdown();
}

#[test]
fn a_grpc_message_limit_of_zero_is_a_start_error() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.grpc_max_message = 0;
    assert!(matches!(Edge::start(c, &budget), Err(EdgeError::Config(_))));
    server.shutdown();
}

#[test]
fn a_certificate_is_loaded_for_grpc_and_only_when_a_tls_or_grpc_listener_needs_it() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let missing = Some(std::path::PathBuf::from("/nonexistent/cert.pem"));
    // grpc alone with a certificate: the certificate is read, so a bad one refuses to start.
    let mut c = config(upstream);
    c.cert = missing.clone();
    c.key = missing.clone();
    assert!(matches!(Edge::start(c, &budget), Err(EdgeError::Tls(_))));
    // h2c alone: no listener uses the certificate, so it is not read.
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = missing.clone();
    c.key = missing;
    Edge::start(c, &budget).expect("starts").shutdown();
    server.shutdown();
}
