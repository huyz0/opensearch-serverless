#![allow(missing_docs)]
//! HTTP/3 connection and stream bounds, teardown, the alt-svc advertisement and abuse.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig};
use support::quic::{self, Pair};
use support::{State, WORKERS, start};

fn config(upstream: SocketAddr, p: &Pair) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h3 = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = Some(p.cert.clone());
    c.key = Some(p.key.clone());
    c.max_connections = 32;
    c
}

async fn served_within(addr: SocketAddr, p: &Pair, secs: u64) -> bool {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(secs))
        .expect("a deadline");
    while Instant::now() < deadline {
        let endpoint = quic::endpoint(p, &[b"h3"]);
        let attempt = tokio::time::timeout(Duration::from_secs(2), async {
            let conn = endpoint.connect(addr, "localhost").ok()?.await.ok()?;
            let (mut driver, mut send) = h3::client::new(h3_quinn::Connection::new(conn))
                .await
                .ok()?;
            tokio::spawn(async move {
                let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
            });
            quic::request(&mut send, "GET", "https://x/", None)
                .await
                .ok()
        })
        .await;
        if attempt.is_ok_and(|a| a.is_some_and(|a| a.status == 200)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

// AC-7
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_resets_or_vanishes_mid_upload_frees_its_stream_slot() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("gone");
    let mut c = config(upstream, &p);
    c.max_streams = 1;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    for how in ["reset the stream", "close the connection"] {
        let endpoint = quic::endpoint(&p, &[b"h3"]);
        let (conn, mut send) = quic::connect(&endpoint, addr).await;
        let request = http::Request::builder()
            .method("POST")
            .uri("https://x/sum")
            .header("content-type", "application/json")
            .header("content-length", "1000000")
            .body(())
            .expect("request");
        let mut stream = send.send_request(request).await.expect("sent");
        stream
            .send_data(Bytes::from(vec![1u8; 1000]))
            .await
            .expect("some body");
        // The one slot is now held by a handler waiting for the rest of the body.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let probe = quic::endpoint(&p, &[b"h3"]);
        let (_c2, mut other) = quic::connect(&probe, addr).await;
        let busy = quic::request(&mut other, "GET", "https://x/", None).await;
        assert!(
            busy.is_err(),
            "the slot is held while the upload waits: {busy:?}"
        );
        if how == "reset the stream" {
            stream.stop_stream(h3::error::Code::H3_REQUEST_CANCELLED);
        } else {
            conn.close(0u32.into(), b"gone");
        }
        assert!(
            served_within(addr, &p, 15).await,
            "after the client {how}, the slot is free again"
        );
    }
    edge.shutdown();
    server.shutdown();
}

// AC-12
#[tokio::test(flavor = "multi_thread")]
async fn the_tls_listener_advertises_h3_only_when_it_is_configured() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("altsvc");
    let name = rustls::pki_types::ServerName::try_from("localhost").expect("name");
    let connector = |p: &Pair| {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(p.der.clone()).expect("root");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut c = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        c.alpn_protocols = vec![b"h2".to_vec()];
        tokio_rustls::TlsConnector::from(Arc::new(c))
    };
    for with_h3 in [true, false] {
        let mut c = config(upstream, &p);
        c.tls = Some("127.0.0.1:0".parse().expect("addr"));
        if !with_h3 {
            c.h3 = None;
        }
        let edge = Edge::start(c, &budget).expect("starts");
        let tcp = tokio::net::TcpStream::connect(edge.tls_addr().expect("bound"))
            .await
            .expect("tcp");
        let tls = connector(&p).connect(name.clone(), tcp).await.expect("tls");
        let (mut send, conn) = h2::client::handshake(tls).await.expect("h2");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let request = http::Request::builder()
            .uri("https://localhost/")
            .body(())
            .expect("request");
        let (response, _) = send.send_request(request, true).expect("sent");
        let response = response.await.expect("answer");
        let alt = response
            .headers()
            .get("alt-svc")
            .map(|v| v.to_str().expect("text").to_owned());
        if with_h3 {
            let port = edge.h3_addr().expect("bound").port();
            assert_eq!(alt, Some(format!("h3=\":{port}\"; ma=86400")));
        } else {
            assert_eq!(alt, None, "no h3 listener, no advertisement");
        }
        edge.shutdown();
    }
    server.shutdown();
}

// AC-16
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_upload_does_not_stall_a_sibling_stream() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("stall");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let endpoint = quic::endpoint(&p, &[b"h3"]);
    let (_conn, mut send) = quic::connect(&endpoint, edge.h3_addr().expect("bound")).await;
    let request = http::Request::builder()
        .method("POST")
        .uri("https://x/sum")
        .header("content-type", "application/json")
        .header("content-length", "1000000")
        .body(())
        .expect("request");
    let mut stalled = send.send_request(request).await.expect("sent");
    stalled
        .send_data(Bytes::from(vec![1u8; 100]))
        .await
        .expect("a start");
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        quic::request(&mut send, "GET", "https://x/", None),
    )
    .await
    .expect("the sibling is not stalled")
    .expect("answer");
    assert_eq!(answer.status, 200);
    drop(stalled);
    edge.shutdown();
    server.shutdown();
}

// AC-15
#[tokio::test(flavor = "multi_thread")]
async fn connections_past_the_limit_are_refused_and_idle_ones_time_out() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("idle");
    let mut c = config(upstream, &p);
    c.max_connections = 1;
    c.idle_timeout = Duration::from_millis(400);
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    let endpoint = quic::endpoint(&p, &[b"h3"]);
    let (conn, mut first) = quic::connect(&endpoint, addr).await;
    assert_eq!(
        quic::request(&mut first, "GET", "https://x/", None)
            .await
            .expect("first")
            .status,
        200
    );
    let second = quic::endpoint(&p, &[b"h3"]);
    let refused = tokio::time::timeout(Duration::from_secs(3), async {
        match second.connect(addr, "localhost").expect("connecting").await {
            Err(_) => true,
            Ok(conn) => {
                let (mut driver, mut send) = h3::client::new(h3_quinn::Connection::new(conn))
                    .await
                    .expect("h3");
                tokio::spawn(async move {
                    let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
                });
                quic::request(&mut send, "GET", "https://x/", None)
                    .await
                    .is_err()
            }
        }
    })
    .await
    .unwrap_or(true);
    assert!(
        refused,
        "a second connection past max_connections is not served"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        conn.close_reason().is_some(),
        "an idle connection is closed after idle_timeout"
    );
    edge.shutdown();
    server.shutdown();
}

// AC-15: a handshake that never finishes holds a connection slot only for `handshake_timeout`.
#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_that_never_finishes_is_dropped() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("handshake");
    let mut c = config(upstream, &p);
    c.max_connections = 1;
    c.handshake_timeout = Duration::from_millis(800);
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");

    // A relay that passes the client's first two datagrams (the Initial, and the Initial that
    // answers the Retry) and then none, so the server accepts a connection and never hears the
    // rest of the handshake.
    let front = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let front_addr = front.local_addr().expect("addr");
    let back = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind");
    back.connect(addr).await.expect("connect");
    let relay = tokio::spawn(async move {
        let mut passed = 0u32;
        let mut client: Option<SocketAddr> = None;
        let (mut a, mut b) = ([0u8; 2048], [0u8; 2048]);
        loop {
            tokio::select! {
                Ok((n, from)) = front.recv_from(&mut a) => {
                    client = Some(from);
                    if passed < 2 {
                        passed = passed.saturating_add(1);
                        let _ = back.send(&a[..n]).await;
                    }
                }
                Ok(n) = back.recv(&mut b) => {
                    if let Some(c) = client {
                        let _ = front.send_to(&b[..n], c).await;
                    }
                }
            }
        }
    });
    let stuck = quic::endpoint(&p, &[b"h3"]);
    let attempt = stuck.connect(front_addr, "localhost").expect("connecting");
    let stuck_task = tokio::spawn(attempt);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let direct = quic::endpoint(&p, &[b"h3"]);
    let during = tokio::time::timeout(Duration::from_millis(400), async {
        let conn = direct
            .connect(addr, "localhost")
            .expect("connecting")
            .await?;
        let (mut driver, mut send) = h3::client::new(h3_quinn::Connection::new(conn))
            .await
            .map_err(|_| quinn::ConnectionError::LocallyClosed)?;
        tokio::spawn(async move {
            let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });
        Ok::<_, quinn::ConnectionError>(
            quic::request(&mut send, "GET", "https://x/", None)
                .await
                .ok(),
        )
    })
    .await;
    assert!(
        !matches!(during, Ok(Ok(Some(_)))),
        "the one slot is held by the unfinished handshake"
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        served_within(addr, &p, 10).await,
        "after handshake_timeout the slot is free"
    );
    relay.abort();
    stuck_task.abort();
    edge.shutdown();
    server.shutdown();
}

// AC-17
#[tokio::test(flavor = "multi_thread")]
async fn stream_floods_and_connection_floods_are_survived() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("flood");
    let mut c = config(upstream, &p);
    c.max_connections = 8;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    // 1,000 streams opened and cancelled in a burst on one connection.
    let endpoint = quic::endpoint(&p, &[b"h3"]);
    let (_conn, mut send) = quic::connect(&endpoint, addr).await;
    for _ in 0..1000 {
        let request = http::Request::builder()
            .uri("https://x/hold")
            .body(())
            .expect("request");
        match send.send_request(request).await {
            Ok(mut s) => s.stop_stream(h3::error::Code::H3_REQUEST_CANCELLED),
            Err(_) => break,
        }
    }
    // More connections than max_connections, opened at once.
    let mut held = Vec::new();
    for _ in 0..24 {
        let e = quic::endpoint(&p, &[b"h3"]);
        if let Ok(connecting) = e.connect(addr, "localhost") {
            held.push((e, tokio::spawn(connecting)));
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    state.release.store(true, Ordering::Release);
    drop(held);
    assert!(
        served_within(addr, &p, 20).await,
        "the node serves the next request after the floods"
    );
    edge.shutdown();
    server.shutdown();
}

// AC-15: a connection with no stream open is closed after `idle_timeout` even when the client keeps
// pinging, and one with a stream open is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_pinging_client_with_no_stream_open_is_closed_and_one_with_a_stream_is_not() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("pinging");
    let mut c = config(upstream, &p);
    c.idle_timeout = Duration::from_millis(500);
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.h3_addr().expect("bound");
    let ping = Some(Duration::from_millis(100));

    // No stream open: closed by the front, though the transport never goes idle.
    let endpoint = quic::endpoint_with(&p, &[b"h3"], ping);
    let (conn, mut send) = quic::connect(&endpoint, addr).await;
    assert_eq!(
        quic::request(&mut send, "GET", "https://x/", None)
            .await
            .expect("served")
            .status,
        200
    );
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        conn.close_reason().is_some(),
        "the front closes a connection with no stream open after idle_timeout"
    );

    // A stream open for longer than `idle_timeout` keeps its connection.
    let endpoint = quic::endpoint_with(&p, &[b"h3"], ping);
    let (conn, mut send) = quic::connect(&endpoint, addr).await;
    let held =
        tokio::spawn(async move { quic::request(&mut send, "GET", "https://x/hold", None).await });
    while state.holding.load(Ordering::Acquire) < 1 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(
        conn.close_reason().is_none(),
        "a connection with a stream open stays"
    );
    state.release.store(true, Ordering::Release);
    assert_eq!(held.await.expect("joins").expect("completes").status, 200);
    edge.shutdown();
    server.shutdown();
}
