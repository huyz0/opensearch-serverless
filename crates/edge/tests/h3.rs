#![allow(missing_docs)]
//! HTTP/3 through the front to a real HTTP/1.1 server.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig, EdgeError, H3_ENDPOINT_BYTES};
use oss_mem::Pool;
use support::quic::{self, Pair, Send};
use support::{MAX_BODY, State, WORKERS, h1, start};

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

// AC-2
#[tokio::test(flavor = "multi_thread")]
async fn answers_match_http1_for_a_set_of_requests() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("diff");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/nothing/here"),
        ("POST", "/"),
        ("DELETE", "/"),
        ("GET", "/%zz"),
    ] {
        let over3 = quic::request(
            &mut send,
            method,
            &format!("https://example:9200{path}"),
            None,
        )
        .await
        .expect("h3 answer");
        let over1 = h1(upstream, method, path, "example:9200", b"");
        assert_eq!(over3.status, over1.status, "{method} {path}");
        assert_eq!(over3.content_type, over1.content_type, "{method} {path}");
        assert_eq!(over3.body, over1.body, "{method} {path}");
    }
    let head = quic::request(&mut send, "HEAD", "https://example:9200/", None)
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

// AC-9
#[tokio::test(flavor = "multi_thread")]
async fn authority_is_host_and_the_front_sets_no_forwarded_for() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("host");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let a = quic::request(&mut send, "GET", "https://shown.example:1234/", None)
        .await
        .expect("answer");
    assert_eq!(a.body, br#"{"host":"shown.example:1234","xff":"-"}"#);
    let b = quic::request_with(
        &mut send,
        "GET",
        "https://shown.example:1234/",
        &[("x-forwarded-for", "9.9.9.9")],
        None,
    )
    .await
    .expect("answer");
    assert_eq!(
        b.body, br#"{"host":"shown.example:1234","xff":"9.9.9.9"}"#,
        "a client's own header passes as sent"
    );
    edge.shutdown();
    server.shutdown();
}

// AC-9
// AC-3
#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_the_limit_is_413_like_over_http1_and_the_connection_survives() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("limit");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let limit = usize::try_from(MAX_BODY).expect("fits");
    let at = quic::request(
        &mut send,
        "POST",
        "https://x/sum",
        Some(Bytes::from(vec![1u8; limit])),
    )
    .await
    .expect("at the limit");
    assert_eq!(at.status, 200);
    let over = Bytes::from(vec![1u8; limit + 1]);
    let status = quic::request(&mut send, "POST", "https://x/sum", Some(over.clone()))
        .await
        .map(|a| a.status);
    assert!(
        status.as_ref().is_ok_and(|s| *s == 413) || status.is_err(),
        "one byte over the limit is 413, or the stream is reset once the answer is out: {status:?}"
    );
    assert_eq!(h1(upstream, "POST", "/sum", "x", &over).status, 413);
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

// AC-4
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_past_max_streams_is_rejected_and_the_rest_complete() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("max");
    let mut c = config(upstream, &p);
    c.max_streams = 2;
    let edge = Edge::start(c, &budget).expect("starts");
    let (_e, _c, send) = client(&edge, &p).await;
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let mut s = send.clone();
        tasks.push(tokio::spawn(async move {
            quic::request(&mut s, "GET", "https://x/hold", None).await
        }));
    }
    while state.holding.load(Ordering::Acquire) < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut extra = send.clone();
    let refused = quic::request(&mut extra, "GET", "https://x/hold", None).await;
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("REJECTED")
            || e.contains("H3_REQUEST_REJECTED")
            || e.contains("0x10b")),
        "the third stream is rejected: {refused:?}"
    );
    state.release.store(true, Ordering::Release);
    for t in tasks {
        assert_eq!(t.await.expect("joins").expect("completes").status, 200);
    }
    let mut after = send.clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match quic::request(&mut after, "GET", "https://x/", None).await {
            Ok(a) => {
                assert_eq!(a.status, 200);
                break;
            }
            Err(e) => assert!(
                std::time::Instant::now() < deadline,
                "never served again: {e}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    edge.shutdown();
    server.shutdown();
}

// AC-5
#[tokio::test(flavor = "multi_thread")]
async fn memory_is_charged_refused_when_short_and_released_and_a_certificate_is_required() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("charge");
    let c = config(upstream, &p);
    let h2_part = {
        let mut plain = c.clone();
        plain.h3 = None;
        plain.charge()
    };
    assert_eq!(
        c.charge(),
        h2_part + c.max_connections * c.h3_connection_overhead + H3_ENDPOINT_BYTES,
        "the QUIC state is charged on top of the HTTP/2 charge"
    );
    let before = budget.in_use(Pool::RequestBodies);
    let edge = Edge::start(c.clone(), &budget).expect("starts");
    assert_eq!(budget.in_use(Pool::RequestBodies) - before, c.charge());
    edge.shutdown();
    assert_eq!(budget.in_use(Pool::RequestBodies), before, "released");

    let short = support::budget(c.charge() - 1);
    assert!(matches!(
        Edge::start(c.clone(), &short),
        Err(EdgeError::Memory(_))
    ));

    for (cert, key) in [(false, true), (true, false), (false, false)] {
        let mut bad = c.clone();
        if !cert {
            bad.cert = None;
        }
        if !key {
            bad.key = None;
        }
        let err = Edge::start(bad, &budget).err().expect("refused");
        assert!(
            matches!(&err, EdgeError::Config(m) if m.contains("h3")),
            "names the setting: {err:?}"
        );
    }
    let mut missing = c.clone();
    missing.cert = Some("/nonexistent/cert.pem".into());
    assert!(matches!(
        Edge::start(missing, &budget),
        Err(EdgeError::Tls(_))
    ));
    server.shutdown();
}

// AC-8
#[tokio::test(flavor = "multi_thread")]
async fn only_a_client_offering_h3_is_served() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("alpn");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    for alpn in [&[b"h2".as_slice()][..], &[b"http/1.1".as_slice()][..]] {
        let endpoint = quic::endpoint(&p, alpn);
        let outcome = endpoint
            .connect(addr, "localhost")
            .expect("connecting")
            .await;
        assert!(outcome.is_err(), "a client without h3 is refused");
    }
    let (_e, conn, mut send) = client(&edge, &p).await;
    assert_eq!(
        quic::request(&mut send, "GET", "https://x/", None)
            .await
            .expect("answer")
            .status,
        200
    );
    drop(conn);
    edge.shutdown();
    server.shutdown();
}

// AC-10
#[tokio::test(flavor = "multi_thread")]
async fn zero_rtt_is_off() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("zerortt");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    let endpoint = quic::endpoint(&p, &[b"h3"]);
    // A first connection, so a session ticket is stored.
    let (conn, mut send) = quic::connect(&endpoint, addr).await;
    assert_eq!(
        quic::request(&mut send, "GET", "https://x/", None)
            .await
            .expect("first")
            .status,
        200
    );
    // Let the server's ticket arrive.
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(send);
    conn.close(0u32.into(), b"done");
    let second = endpoint.connect(addr, "localhost").expect("connecting");
    match second.into_0rtt() {
        Ok(_) => panic!("early data must not be possible"),
        Err(connecting) => {
            let conn = connecting.await.expect("a full handshake still completes");
            assert!(
                conn.handshake_data().is_some(),
                "served after a 1-RTT handshake"
            );
        }
    }
    edge.shutdown();
    server.shutdown();
}

// AC-11
#[tokio::test(flavor = "multi_thread")]
async fn an_unvalidated_address_is_sent_a_retry_and_then_served() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("retry");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    assert_eq!(edge.h3_retries(), 0);
    let (_e, _c, mut send) = client(&edge, &p).await;
    assert_eq!(edge.h3_retries(), 1, "one Retry for the one new client");
    assert_eq!(
        quic::request(&mut send, "GET", "https://x/", None)
            .await
            .expect("served after the retry")
            .status,
        200
    );
    edge.shutdown();
    server.shutdown();
}

// AC-14
#[tokio::test(flavor = "multi_thread")]
async fn a_large_response_streams_and_head_carries_no_body() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("big");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let (_e, _c, mut send) = client(&edge, &p).await;
    let a = quic::request(&mut send, "GET", "https://x/big", None)
        .await
        .expect("answer");
    assert_eq!((a.status, a.body.len()), (200, 8 << 20));
    let head = quic::request(&mut send, "HEAD", "https://x/big", None)
        .await
        .expect("head");
    assert!(head.body.is_empty());
    edge.shutdown();
    server.shutdown();
}

// AC-1
#[tokio::test(flavor = "multi_thread")]
async fn with_no_h3_address_nothing_is_bound_or_charged() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("off");
    let mut c = config(upstream, &p);
    c.h3 = None;
    c.tls = Some("127.0.0.1:0".parse().expect("addr"));
    let before = budget.in_use(Pool::RequestBodies);
    let edge = Edge::start(c.clone(), &budget).expect("starts");
    assert_eq!(edge.h3_addr(), None, "no UDP socket");
    assert_eq!(
        budget.in_use(Pool::RequestBodies) - before,
        c.charge(),
        "the charge is the TCP listeners' alone"
    );
    assert_eq!(
        c.charge(),
        {
            let mut with = c.clone();
            with.h3 = Some("127.0.0.1:0".parse().expect("addr"));
            with.charge()
        } - c.max_connections * c.h3_connection_overhead
            - H3_ENDPOINT_BYTES,
        "HTTP/3 adds exactly its own charge"
    );
    edge.shutdown();
    server.shutdown();
}
