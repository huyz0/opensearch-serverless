#![allow(missing_docs)]
//! docs/specs/http2-front AC-7: a client that drops its TCP connection or
//! sends GOAWAY in the middle of an upload frees its loopback connection, and
//! the node's open count returns to what it was.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig};
use support::{State, WORKERS, start};

fn config(upstream: std::net::SocketAddr) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    c
}

/// Waits until `done` holds, or fails after `limit`.
async fn within(limit: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Opens an upload to `/sum`, sends part of its body and leaves the stream open; the connection's
/// driver task and the open stream.
async fn half_an_upload(
    edge: &Edge,
    server: &oss_rest::Server,
    before: usize,
) -> (tokio::task::JoinHandle<()>, h2::SendStream<Bytes>) {
    let tcp = tokio::net::TcpStream::connect(edge.h2c_addr().expect("bound"))
        .await
        .expect("connects");
    let (mut send, conn) = h2::client::handshake(tcp).await.expect("handshake");
    let request = http::Request::builder()
        .method("POST")
        .uri("http://x/sum")
        .header("content-type", "application/json")
        .body(())
        .expect("request");
    let (_response, mut stream) = send.send_request(request, false).expect("sent");
    stream
        .send_data(Bytes::from(vec![7u8; 4096]), false)
        .expect("a first part of the body");
    let driver = tokio::spawn(async move {
        let _keep = send;
        let _ = conn.await;
    });
    within(
        Duration::from_secs(10),
        "the upload never reached the node",
        || server.open_connections() > before,
    )
    .await;
    (driver, stream)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_drops_its_connection_mid_upload_frees_the_node() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let before = server.open_connections();
    let (driver, _stream) = half_an_upload(&edge, &server, before).await;
    // The driver owns the connection: aborting it drops the TCP stream with no frame.
    driver.abort();
    let at = Instant::now();
    within(
        Duration::from_secs(20),
        "the node's open count never returned",
        || server.open_connections() == before,
    )
    .await;
    eprintln!("freed in {:?}", at.elapsed());
    edge.shutdown();
    server.shutdown();
}

/// One HTTP/2 frame: length, type, flags, stream, payload.
fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).expect("small");
    let mut out = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_sends_goaway_mid_upload_and_leaves_frees_the_node() {
    use tokio::io::AsyncWriteExt;
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let before = server.open_connections();
    let mut tcp = tokio::net::TcpStream::connect(edge.h2c_addr().expect("bound"))
        .await
        .expect("connects");
    // The preface, empty settings, a POST to /sum with the body open and a first 4 KiB of it.
    let mut wire = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    wire.extend(frame(4, 0, 0, &[]));
    let mut block = vec![0x83, 0x86, 0x04, 4];
    block.extend_from_slice(b"/sum");
    block.extend_from_slice(&[0x01, 1, b'x']);
    block.extend_from_slice(&[0x0f, 0x10, 16]);
    block.extend_from_slice(b"application/json");
    wire.extend(frame(1, 4, 1, &block));
    wire.extend(frame(0, 0, 1, &vec![7u8; 4096]));
    tcp.write_all(&wire).await.expect("sent");
    within(
        Duration::from_secs(10),
        "the upload never reached the node",
        || server.open_connections() > before,
    )
    .await;
    // GOAWAY: last stream 0, no error.
    tcp.write_all(&frame(7, 0, 0, &[0; 8]))
        .await
        .expect("goaway");
    tcp.shutdown().await.expect("the client leaves");
    let at = Instant::now();
    within(
        Duration::from_secs(20),
        "the node's open count never returned",
        || server.open_connections() == before,
    )
    .await;
    eprintln!("freed in {:?}", at.elapsed());
    edge.shutdown();
    server.shutdown();
}
