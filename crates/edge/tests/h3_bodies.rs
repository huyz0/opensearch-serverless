#![allow(missing_docs)]
//! HTTP/3 request bodies and the requests the front refuses on their own account.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig};
use support::quic::{self, Pair, Send};
use support::{State, WORKERS, start};

fn config(upstream: SocketAddr, p: &Pair) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h3 = Some("127.0.0.1:0".parse().expect("addr"));
    c.max_connections = 32;
    c.cert = Some(p.cert.clone());
    c.key = Some(p.key.clone());
    c
}

async fn client(edge: &Edge, p: &Pair) -> (quinn::Endpoint, quinn::Connection, Send) {
    let endpoint = quic::endpoint(p, &[b"h3"]);
    let (conn, send) = quic::connect(&endpoint, edge.h3_addr().expect("bound")).await;
    (endpoint, conn, send)
}

#[tokio::test(flavor = "multi_thread")]
async fn bodies_are_bridged_and_a_wrong_length_is_not_served_as_complete() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("body");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let body = Bytes::from(vec![7u8; 768 << 10]);
    let a = quic::request(&mut send, "POST", "https://x/sum", Some(body.clone()))
        .await
        .expect("answer");
    assert_eq!(
        (a.status, a.body.as_slice()),
        (200, &br#"{"bytes":786432}"#[..])
    );
    let with_length = quic::request_with(
        &mut send,
        "POST",
        "https://x/sum",
        &[("content-length", &body.len().to_string())],
        Some(body.clone()),
    )
    .await
    .expect("answer");
    assert_eq!(
        (with_length.status, with_length.body.as_slice()),
        (200, &br#"{"bytes":786432}"#[..]),
    );
    let short = quic::request_with(
        &mut send,
        "POST",
        "https://x/sum",
        &[("content-length", "10")],
        Some(Bytes::from_static(b"short")),
    )
    .await;
    assert!(
        short.as_ref().map_or(true, |a| a.status != 200),
        "a short body must not be served as a complete one: {short:?}"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_length_and_an_oversized_head_are_refused_by_the_front() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("refuse");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let bad = quic::request_with(
        &mut send,
        "POST",
        "https://x/sum",
        &[("content-length", "not-a-number")],
        None,
    )
    .await
    .expect("answered");
    assert_eq!(bad.status, 400);
    let long = format!("https://x/{}", "a".repeat(5000));
    let big = quic::request(&mut send, "GET", &long, None)
        .await
        .expect("answered");
    assert_eq!(big.status, 431, "a request line past 4 KiB");
    assert_eq!(
        quic::request(&mut send, "GET", "https://x/", None)
            .await
            .expect("served after")
            .status,
        200
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_data_frame_does_not_end_a_body_early() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("empty");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let request = http::Request::builder()
        .method("POST")
        .uri("https://x/sum")
        .header("content-type", "application/json")
        .body(())
        .expect("request");
    let mut stream = send.send_request(request).await.expect("sent");
    stream
        .send_data(Bytes::from_static(b"abc"))
        .await
        .expect("data");
    stream.send_data(Bytes::new()).await.expect("empty");
    stream
        .send_data(Bytes::from_static(b"defg"))
        .await
        .expect("data");
    stream.finish().await.expect("finished");
    let response = stream.recv_response().await.expect("response");
    assert_eq!(response.status(), 200);
    let mut body = Vec::new();
    while let Some(mut d) = stream.recv_data().await.expect("data") {
        let n = bytes::Buf::remaining(&d);
        body.extend_from_slice(&bytes::Buf::copy_to_bytes(&mut d, n));
    }
    assert_eq!(body, br#"{"bytes":7}"#);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_of_empty_data_frames_does_not_end_a_body_early() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("empties");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let request = http::Request::builder()
        .method("POST")
        .uri("https://x/sum")
        .header("content-type", "application/json")
        .body(())
        .expect("request");
    let mut stream = send.send_request(request).await.expect("sent");
    stream
        .send_data(Bytes::from_static(b"abc"))
        .await
        .expect("data");
    for _ in 0..40 {
        stream.send_data(Bytes::new()).await.expect("empty");
    }
    stream
        .send_data(Bytes::from_static(b"defg"))
        .await
        .expect("data");
    stream.finish().await.expect("finished");
    let response = stream.recv_response().await.expect("response");
    assert_eq!(response.status(), 200);
    let mut body = Vec::new();
    while let Some(mut d) = stream.recv_data().await.expect("data") {
        let n = bytes::Buf::remaining(&d);
        body.extend_from_slice(&bytes::Buf::copy_to_bytes(&mut d, n));
    }
    assert_eq!(body, br#"{"bytes":7}"#);
    edge.shutdown();
    server.shutdown();
}

/// The handler gets the bytes the client sent, whether the request declared a length or not:
/// neither is chunk-framed wrongly nor cut.
#[tokio::test(flavor = "multi_thread")]
async fn the_handler_receives_exactly_the_bytes_sent() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("exact");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let mut ndjson = Vec::new();
    for i in 0..3000 {
        ndjson.extend_from_slice(
            format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n{{\"n\":{i}}}\n").as_bytes(),
        );
    }
    for declared in [false, true] {
        let length = ndjson.len().to_string();
        let headers: Vec<(&str, &str)> = if declared {
            vec![("content-length", length.as_str())]
        } else {
            vec![]
        };
        let a = quic::request_with(
            &mut send,
            "POST",
            "https://x/_bulk",
            &headers,
            Some(Bytes::from(ndjson.clone())),
        )
        .await
        .expect("answer");
        assert_eq!(a.status, 200, "declared length: {declared}");
    }
    let seen = state.bulks.lock().expect("lock");
    assert_eq!(seen.len(), 2);
    for (i, (_target, body)) in seen.iter().enumerate() {
        assert_eq!(body, &ndjson, "request {i}: the bytes the client sent");
    }
    drop(seen);
    edge.shutdown();
    server.shutdown();
}
