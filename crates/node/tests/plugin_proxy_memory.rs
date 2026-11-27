//! docs/specs/plugin-proxy AC-7: a body of 1 GiB streams through the proxy,
//! in and out, and the heap peak stays far under it. Its own binary, because
//! the counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::plugin_proxy::{PluginProxy, ProxySettings};
use oss_rest::{Router, Server, ServerConfig};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

/// The body each way: 1 GiB.
const BODY: u64 = 1 << 30;
/// The repeating block the body is made of, so no side holds the body.
const BLOCK: usize = 64 << 10;
/// The request-body pool the server is built from.
const POOL: usize = 8 << 20;
/// What the whole process may hold at its peak, both directions in flight:
/// the pool, the proxy's chunk and the client and upstream buffers. A
/// body held whole would be 1 GiB.
const BOUND: usize = 16 << 20;

fn block() -> Vec<u8> {
    (0..BLOCK)
        .map(|i| u8::try_from(i % 251).expect("byte"))
        .collect()
}

/// Reads until `\r\n\r\n`, returns the head and the body bytes already read.
fn read_head(s: &mut TcpStream) -> (String, Vec<u8>) {
    let mut all = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf).expect("reads");
        assert!(n > 0, "closed inside the head");
        all.extend_from_slice(&buf[..n]);
        if let Some(p) = all.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = all.split_off(p + 4);
            return (String::from_utf8_lossy(&all).into_owned(), rest);
        }
    }
}

/// Checks `piece`, which starts at stream offset `at`, against the block.
fn check(block: &[u8], at: u64, piece: &[u8]) {
    let mut done = 0;
    while done < piece.len() {
        let off = usize::try_from((at + done as u64) % BLOCK as u64).expect("fits");
        let n = (BLOCK - off).min(piece.len() - done);
        assert_eq!(&piece[done..done + n], &block[off..off + n], "at {at}");
        done += n;
    }
}

/// A fake Java node: counts and checks the request body, then answers with
/// a body of the same size made of the same block.
fn upstream(block: Arc<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            let block = Arc::clone(&block);
            thread::spawn(move || {
                let (head, first) = read_head(&mut s);
                let lower = head.to_ascii_lowercase();
                assert!(lower.contains(&format!("content-length: {BODY}")), "{head}");
                let mut got = first.len() as u64;
                check(&block, 0, &first);
                let mut buf = vec![0u8; BLOCK];
                while got < BODY {
                    let n = s.read(&mut buf).expect("body");
                    assert!(n > 0, "request body cut at {got}");
                    check(&block, got, &buf[..n]);
                    got += n as u64;
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {BODY}\r\nconnection: close\r\n\r\n"
                );
                s.write_all(head.as_bytes()).expect("head");
                for _ in 0..(BODY / BLOCK as u64) {
                    s.write_all(&block).expect("answer");
                }
            });
        }
    });
    base
}

#[test]
fn a_gibibyte_streams_through_the_proxy_within_the_pool() {
    let block = Arc::new(block());
    let base = upstream(Arc::clone(&block));
    let proxy = Arc::new(
        PluginProxy::new(&ProxySettings {
            upstreams: vec![base],
            max_request_bytes: BODY,
            timeout: Duration::from_secs(60),
        })
        .expect("proxy"),
    );
    let router = proxy.register(Router::new()).expect("register");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(POOL)
            .with_pool(Pool::RequestBodies, POOL)
            .expect("fits"),
    );
    let config = ServerConfig {
        workers: 2,
        max_connections: 4,
        max_content_length: BODY,
        body_buffer_size: BLOCK,
        body_buffers: 2,
        write_buffer_size: BLOCK,
        ..ServerConfig::default()
    };
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        config,
        router,
        &budget,
    )
    .expect("starts");
    let mut client = TcpStream::connect(server.local_addr()).expect("connects");
    client
        .set_read_timeout(Some(Duration::from_secs(120)))
        .expect("timeout");

    let window = PeakWindow::start(&ALLOC);
    let sender = {
        let mut out = client.try_clone().expect("clone");
        let block = Arc::clone(&block);
        thread::spawn(move || {
            let head = format!(
                "POST /_plugins/_ml/models/_upload HTTP/1.1\r\nHost: client.example\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {BODY}\r\n\r\n"
            );
            out.write_all(head.as_bytes()).expect("head");
            for _ in 0..(BODY / BLOCK as u64) {
                out.write_all(&block).expect("body");
            }
        })
    };
    let (head, first) = read_head(&mut client);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut got = first.len() as u64;
    check(&block, 0, &first);
    let mut buf = vec![0u8; BLOCK];
    while got < BODY {
        let n = client.read(&mut buf).expect("answer");
        assert!(n > 0, "answer cut at {got}");
        check(&block, got, &buf[..n]);
        got += n as u64;
    }
    sender.join().expect("sender");
    let peak = window.peak_bytes();
    server.shutdown();
    println!("1 GiB each way; heap peak {peak} bytes; pool {POOL}, bound {BOUND}");
    assert_eq!(got, BODY);
    assert!(peak <= BOUND, "peak {peak} over {BOUND}");
}
