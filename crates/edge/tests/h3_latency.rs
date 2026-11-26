#![allow(missing_docs)]
//! AC-19: latency of a small GET over HTTP/3, HTTP/2 over TLS and HTTP/1.1, one connection each,
//! measured in one process on loopback. A record of relative cost, not a threshold.

mod support;

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_edge::{Edge, EdgeConfig};
use support::quic;
use support::{State, WORKERS, h2_request, start};

const REQUESTS: usize = 2000;

fn percentiles(mut micros: Vec<u128>) -> (u128, u128) {
    micros.sort_unstable();
    (
        micros[micros.len() / 2],
        micros[micros.len().saturating_mul(99) / 100],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn latency_of_a_small_get_over_each_protocol_is_recorded() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = quic::pair("latency");
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h3 = Some("127.0.0.1:0".parse().expect("addr"));
    c.tls = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = Some(p.cert.clone());
    c.key = Some(p.key.clone());
    c.max_connections = 32;
    let edge = Edge::start(c, &budget).expect("starts");

    let endpoint = quic::endpoint(&p, &[b"h3"]);
    let (_conn, mut send3) = quic::connect(&endpoint, edge.h3_addr().expect("bound")).await;
    let mut h3 = Vec::with_capacity(REQUESTS);
    for _ in 0..REQUESTS {
        let t = Instant::now();
        let a = quic::request(&mut send3, "GET", "https://x/", None)
            .await
            .expect("h3");
        assert_eq!(a.status, 200);
        h3.push(t.elapsed().as_micros());
    }

    let mut roots = rustls::RootCertStore::empty();
    roots.add(p.der.clone()).expect("root");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cc = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    cc.alpn_protocols = vec![b"h2".to_vec()];
    let tcp = tokio::net::TcpStream::connect(edge.tls_addr().expect("bound"))
        .await
        .expect("tcp");
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cc))
        .connect(
            rustls::pki_types::ServerName::try_from("localhost").expect("name"),
            tcp,
        )
        .await
        .expect("tls");
    let (mut send2, conn2) = h2::client::handshake(tls).await.expect("h2");
    tokio::spawn(async move {
        let _ = conn2.await;
    });
    let mut h2v = Vec::with_capacity(REQUESTS);
    for _ in 0..REQUESTS {
        let t = Instant::now();
        let a = h2_request(&mut send2, "GET", "https://x/", None)
            .await
            .expect("h2");
        assert_eq!(a.status, 200);
        h2v.push(t.elapsed().as_micros());
    }

    let mut s = std::net::TcpStream::connect(upstream).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut h1 = Vec::with_capacity(REQUESTS);
    let mut buf = [0u8; 4096];
    for _ in 0..REQUESTS {
        let t = Instant::now();
        s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
            .expect("request");
        let mut got = Vec::new();
        loop {
            let n = s.read(&mut buf).expect("answer");
            got.extend_from_slice(&buf[..n]);
            if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
                let want: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if got.len() >= end + 4 + want {
                    break;
                }
            }
        }
        h1.push(t.elapsed().as_micros());
    }

    let (a50, a99) = percentiles(h3);
    let (b50, b99) = percentiles(h2v);
    let (c50, c99) = percentiles(h1);
    println!(
        "H3LATENCY|{REQUESTS} sequential GETs, one connection each, microseconds p50/p99: http3 {a50}/{a99}, http2 over tls {b50}/{b99}, http1.1 {c50}/{c99}; provisional: debug build, loopback"
    );
    edge.shutdown();
    server.shutdown();
}
