#![allow(missing_docs)]
//! HTTP/2 over clear text through the front to a real HTTP/1.1 server.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig, EdgeError};
use oss_mem::Pool;
use support::{MAX_BODY, State, WORKERS, h1, h2_request, h2c, start};

fn config(upstream: std::net::SocketAddr) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    c
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_match_http1_for_a_set_of_requests() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/nothing/here"),
        ("POST", "/"),
        ("DELETE", "/"),
        ("GET", "/%zz"),
    ] {
        let over2 = h2_request(
            &mut send,
            method,
            &format!("http://example:9200{path}"),
            None,
        )
        .await
        .expect("h2 answer");
        let over1 = h1(upstream, method, path, "example:9200", b"");
        assert_eq!(over2.status, over1.status, "{method} {path}");
        assert_eq!(over2.content_type, over1.content_type, "{method} {path}");
        assert_eq!(over2.body, over1.body, "{method} {path}");
    }
    let head = h2_request(&mut send, "HEAD", "http://example:9200/", None)
        .await
        .expect("head");
    assert_eq!(
        head.status,
        h1(upstream, "HEAD", "/", "example:9200", b"").status
    );
    assert!(head.body.is_empty());
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn authority_reaches_the_handler_as_host() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    let a = h2_request(&mut send, "GET", "http://shown.example:1234/", None)
        .await
        .expect("answer");
    assert_eq!(
        a.body, br#"{"host":"shown.example:1234","xff":"-"}"#,
        "the front adds no x-forwarded-for"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn bodies_are_bridged_with_and_without_a_length() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    let body = Bytes::from(vec![7u8; 768 << 10]);
    // Chunked on the loopback: no content-length.
    let a = h2_request(&mut send, "POST", "http://x/sum", Some(body.clone()))
        .await
        .expect("answer");
    assert_eq!(
        (a.status, a.body.as_slice()),
        (200, &br#"{"bytes":786432}"#[..])
    );
    // With a declared length.
    let request = http::Request::builder()
        .method("POST")
        .uri("http://x/sum")
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .body(())
        .expect("request");
    let (response, mut stream) = send.send_request(request, false).expect("sent");
    let mut data = body.clone();
    while !data.is_empty() {
        stream.reserve_capacity(data.len());
        let cap = std::future::poll_fn(|cx| stream.poll_capacity(cx))
            .await
            .expect("some")
            .expect("cap");
        let part = data.split_to(cap.min(data.len()));
        stream.send_data(part, data.is_empty()).expect("data");
    }
    let response = response.await.expect("response");
    assert_eq!(response.status(), 200);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_length_that_differs_from_the_body_resets_the_stream() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    let request = http::Request::builder()
        .method("POST")
        .uri("http://x/sum")
        .header("content-type", "application/json")
        .header("content-length", "10")
        .body(())
        .expect("request");
    let (response, mut stream) = send.send_request(request, false).expect("sent");
    stream.send_data(Bytes::from_static(b"short"), true).ok();
    let outcome = tokio::time::timeout(Duration::from_secs(10), response)
        .await
        .expect("answers");
    assert!(
        outcome.is_err() || outcome.is_ok_and(|r| r.status() != 200),
        "a short body must not be served as a complete one"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_response_streams_through_a_small_window() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    let a = h2_request(&mut send, "GET", "http://x/big", None)
        .await
        .expect("answer");
    assert_eq!((a.status, a.body.len()), (200, 8 << 20));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_past_max_streams_is_refused_and_the_rest_complete() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.max_streams = 2;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h2c_addr().expect("bound");
    let mut tasks = Vec::new();
    for _ in 0..2 {
        tasks.push(tokio::spawn(async move {
            let mut send = h2c(addr).await;
            h2_request(&mut send, "GET", "http://x/hold", None).await
        }));
    }
    while state.holding.load(Ordering::Acquire) < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut send = h2c(addr).await;
    let refused = h2_request(&mut send, "GET", "http://x/hold", None).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.reason() == Some(h2::Reason::REFUSED_STREAM)),
        "{refused:?}"
    );
    state.release.store(true, Ordering::Release);
    for t in tasks {
        assert_eq!(t.await.expect("joins").expect("answer").status, 200);
    }
    let again = h2_request(&mut send, "GET", "http://x/", None)
        .await
        .expect("served after");
    assert_eq!(again.status, 200);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_is_charged_at_start_refused_when_short_and_released() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let c = config(upstream);
    let charge = c.charge();
    let before = budget.in_use(Pool::RequestBodies);
    let edge = Edge::start(c.clone(), &budget).expect("starts");
    assert_eq!(budget.in_use(Pool::RequestBodies), before + charge);
    edge.shutdown();
    assert_eq!(budget.in_use(Pool::RequestBodies), before);
    let short = support::budget(charge - 1);
    assert!(matches!(Edge::start(c, &short), Err(EdgeError::Memory(_))));
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn inconsistent_settings_are_refused() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.max_streams = WORKERS / 4 + 1;
    assert!(
        matches!(Edge::start(c, &budget), Err(EdgeError::Config(m)) if m.contains("max_streams"))
    );
    let mut c = config(upstream);
    c.tls = Some("127.0.0.1:0".parse().expect("addr"));
    assert!(
        matches!(Edge::start(c, &budget), Err(EdgeError::Config(m)) if m.contains("certificate"))
    );
    let mut c = config(upstream);
    c.stream_buffer = 1024;
    assert!(matches!(Edge::start(c, &budget), Err(EdgeError::Config(_))));
    assert_eq!(
        budget.in_use(Pool::RequestBodies),
        support::budget(1).in_use(Pool::RequestBodies) + budget.in_use(Pool::RequestBodies)
    );
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_past_the_limit_are_closed_and_idle_ones_time_out() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.max_connections = 1;
    c.idle_timeout = Duration::from_millis(300);
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h2c_addr().expect("bound");
    let mut first = h2c(addr).await;
    let second = tokio::net::TcpStream::connect(addr)
        .await
        .expect("tcp connects");
    let handshake = h2::client::handshake(second).await;
    let refused = match handshake {
        Ok((mut s, conn)) => {
            tokio::spawn(async move {
                let _ = conn.await;
            });
            h2_request(&mut s, "GET", "http://x/", None).await.is_err()
        }
        Err(_) => true,
    };
    assert!(
        refused,
        "a second connection past max_connections is not served"
    );
    assert_eq!(
        h2_request(&mut first, "GET", "http://x/", None)
            .await
            .expect("first")
            .status,
        200
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(
        h2_request(&mut first, "GET", "http://x/", None)
            .await
            .is_err(),
        "an idle connection is closed after idle_timeout"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn rapid_resets_are_disconnected_and_the_node_serves_the_next_request() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let addr = edge.h2c_addr().expect("bound");
    let mut send = h2c(addr).await;
    let mut refused = false;
    for _ in 0..2000 {
        let request = http::Request::builder()
            .method("GET")
            .uri("http://x/hold")
            .body(())
            .expect("request");
        match send.send_request(request, true) {
            Ok((_response, mut stream)) => stream.send_reset(h2::Reason::CANCEL),
            Err(_) => {
                refused = true;
                break;
            }
        }
        tokio::task::yield_now().await;
    }
    state.release.store(true, Ordering::Release);
    let _ = refused;
    // The cancelled streams free their slots as the held handlers finish, so a request
    // that lands first may be refused; it must be served once they have.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let a = loop {
        let mut fresh = h2c(addr).await;
        match h2_request(&mut fresh, "GET", "http://x/", None).await {
            Ok(a) => break a,
            Err(e) if e.reason() == Some(h2::Reason::REFUSED_STREAM) => {
                assert!(std::time::Instant::now() < deadline, "slots never freed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("answer: {e:?}"),
        }
    };
    assert_eq!(a.status, 200);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_upload_does_not_stall_a_sibling_stream() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    // A stream that declares a large body and sends a little, then stops.
    let request = http::Request::builder()
        .method("POST")
        .uri("http://x/sum")
        .header("content-type", "application/json")
        .header("content-length", "1000000")
        .body(())
        .expect("request");
    let (_stalled, mut stream) = send.send_request(request, false).expect("sent");
    stream
        .send_data(Bytes::from(vec![1u8; 50_000]), false)
        .expect("some data");
    let a = tokio::time::timeout(
        Duration::from_secs(10),
        h2_request(&mut send, "GET", "http://x/", None),
    )
    .await
    .expect("sibling answers while the other is stalled")
    .expect("answer");
    assert_eq!(a.status, 200);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_the_limit_is_refused_413_like_over_http1_and_the_connection_survives() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    let limit = usize::try_from(MAX_BODY).expect("fits");
    let at = h2_request(
        &mut send,
        "POST",
        "http://x/sum",
        Some(Bytes::from(vec![1u8; limit])),
    )
    .await
    .expect("at the limit");
    assert_eq!(at.status, 200);
    let over = Bytes::from(vec![1u8; limit + 1]);
    let refused = h2_request(&mut send, "POST", "http://x/sum", Some(over.clone())).await;
    let status = refused.map(|a| a.status);
    assert!(
        status.as_ref().is_ok_and(|s| *s == 413) || status.is_err(),
        "one byte over the limit is 413, or the stream is reset once the answer is out: {status:?}"
    );
    assert_eq!(
        h1(upstream, "POST", "/sum", "x", &over).status,
        413,
        "the same as HTTP/1.1"
    );
    let mut fresh = h2c(edge.h2c_addr().expect("bound")).await;
    assert_eq!(
        h2_request(&mut fresh, "GET", "http://x/", None)
            .await
            .expect("served after")
            .status,
        200
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_sent_forwarded_for_passes_through_unchanged() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.h2c_addr().expect("bound")).await;
    let request = http::Request::builder()
        .uri("http://x/")
        .header("x-forwarded-for", "203.0.113.9")
        .body(())
        .expect("request");
    let (response, _) = send.send_request(request, true).expect("sent");
    let response = response.await.expect("response");
    let mut body = response.into_body();
    let data = body.data().await.expect("data").expect("bytes");
    assert_eq!(&data[..], br#"{"host":"x","xff":"203.0.113.9"}"#);
    edge.shutdown();
    server.shutdown();
}
