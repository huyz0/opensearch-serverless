#![allow(missing_docs)]
//! TLS with ALPN: `h2` for an HTTP/2 client, `http/1.1` for the rest, and refusals at start.

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use oss_edge::{Edge, EdgeConfig, EdgeError};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use support::{State, WORKERS, h2_request, start};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsConnector;

struct Pair {
    cert: PathBuf,
    key: PathBuf,
    der: rustls::pki_types::CertificateDer<'static>,
}

fn pair(tag: &str) -> Pair {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
    let dir = std::env::temp_dir().join(format!("oss-edge-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, made.cert.pem()).expect("cert file");
    std::fs::write(&key, made.key_pair.serialize_pem()).expect("key file");
    Pair {
        cert,
        key,
        der: made.cert.der().clone(),
    }
}

fn config(upstream: std::net::SocketAddr, p: &Pair) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.tls = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = Some(p.cert.clone());
    c.key = Some(p.key.clone());
    c
}

fn connector(p: &Pair, alpn: &[&[u8]]) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.add(p.der.clone()).expect("root");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut c = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    c.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    TlsConnector::from(Arc::new(c))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_h2_client_gets_h2_and_an_http1_client_gets_http1_with_the_same_answer() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = pair("alpn");
    let edge = Edge::start(config(upstream, &p), &budget).expect("starts");
    let addr = edge.tls_addr().expect("bound");
    let name = ServerName::try_from("localhost").expect("name");

    let tcp = tokio::net::TcpStream::connect(addr).await.expect("tcp");
    let tls = connector(&p, &[b"h2", b"http/1.1"])
        .connect(name.clone(), tcp)
        .await
        .expect("tls");
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
    let (mut send, conn) = h2::client::handshake(tls).await.expect("h2");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let over2 = h2_request(&mut send, "GET", "https://localhost/", None)
        .await
        .expect("h2 answer");

    let tcp = tokio::net::TcpStream::connect(addr).await.expect("tcp");
    let mut tls = connector(&p, &[b"http/1.1"])
        .connect(name, tcp)
        .await
        .expect("tls");
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );
    tls.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut raw = Vec::new();
    let _ = tls.read_to_end(&mut raw).await;
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert!(text.starts_with("HTTP/1.1 200 "), "{text}");
    assert!(
        text.ends_with(std::str::from_utf8(&over2.body).expect("utf8")),
        "{text}"
    );
    assert_eq!(over2.status, 200);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plain_client_to_the_tls_port_is_not_served() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = pair("plain");
    let mut c = config(upstream, &p);
    c.handshake_timeout = std::time::Duration::from_millis(300);
    let edge = Edge::start(c, &budget).expect("starts");
    let mut tcp = tokio::net::TcpStream::connect(edge.tls_addr().expect("bound"))
        .await
        .expect("tcp");
    tcp.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
        .await
        .expect("write");
    let mut raw = Vec::new();
    let _ =
        tokio::time::timeout(std::time::Duration::from_secs(5), tcp.read_to_end(&mut raw)).await;
    assert!(
        !String::from_utf8_lossy(&raw).contains("200 OK"),
        "clear text must not reach the server"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_unreadable_or_mismatched_certificate_refuses_to_start() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let p = pair("good");
    let other = pair("other");
    let mut missing = config(upstream, &p);
    missing.cert = Some(p.cert.with_file_name("absent.pem"));
    let mut mismatched = config(upstream, &p);
    mismatched.key = Some(other.key.clone());
    let mut empty = config(upstream, &p);
    let blank = p.cert.with_file_name("blank.pem");
    std::fs::write(&blank, "").expect("blank");
    empty.cert = Some(blank);
    for (name, c) in [
        ("missing", missing),
        ("mismatched", mismatched),
        ("empty", empty),
    ] {
        let r = Edge::start(c, &budget);
        assert!(
            matches!(&r, Err(EdgeError::Tls(m)) if m.contains("edge")),
            "{name}: {:?}",
            r.err()
        );
    }
    server.shutdown();
}
