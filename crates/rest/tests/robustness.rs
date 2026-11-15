//! Slow, stalled and misbehaving peers and handlers: pauses, trickles,
//! panics, and the edge cases where OpenSearch's answer is specific.

mod support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use oss_rest::{JSON, Length, Method, Request, Response, RestError, Router, ServerConfig};
use serde_json::json;
use support::{Client, small, start};

/// Reads the whole body and answers with its length.
fn count(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let mut bytes = 0usize;
    while let Some(c) = req.body().next_chunk()? {
        bytes = bytes.saturating_add(c.len());
    }
    resp.send(200, JSON, json!({"bytes": bytes}).to_string().as_bytes())
}

fn ok(_: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    resp.send(200, JSON, b"{}")
}

fn router() -> Router {
    Router::new()
        .route(Method::Post, "/count", &[], count)
        .and_then(|r| r.route(Method::Get, "/ok", &["known"], ok))
        .and_then(|r| r.route(Method::Post, "/ok", &["known"], ok))
        .expect("routes")
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("time");
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_body_sent_after_a_pause_longer_than_the_idle_poll_is_read() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let head =
        b"POST /count HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n";
    c.send(head).expect("sends");
    thread::sleep(Duration::from_millis(150));
    let r = c.request(b"{}");
    assert_eq!((r.status, r.json()), (200, json!({"bytes": 2})));

    let head = b"POST /count HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: 2\r\nexpect: 100-continue\r\n\r\n";
    c.send(head).expect("sends");
    assert_eq!(c.reply().expect("100").status, 100);
    thread::sleep(Duration::from_millis(150));
    let r = c.request(b"{}");
    assert_eq!((r.status, r.json()), (200, json!({"bytes": 2})));
    server.shutdown();
}

#[test]
fn a_panicking_handler_answers_500_and_keeps_its_worker() {
    let router = Router::new()
        .route(
            Method::Get,
            "/panic",
            &[],
            |_: &mut Request<'_>, _: &mut Response<'_>| -> Result<(), RestError> {
                panic!("a handler bug")
            },
        )
        .and_then(|r| r.route(Method::Get, "/ok", &[], ok))
        .expect("routes");
    let config = ServerConfig {
        workers: 1,
        max_connections: 4,
        ..small()
    };
    let server = start(config, router);
    for _ in 0..3 {
        let mut c = Client::connect(server.local_addr());
        let r = c.request(b"GET /panic HTTP/1.1\r\n\r\n");
        assert_eq!(r.status, 500);
        assert_eq!(r.header("connection"), Some("close"));
        assert!(c.is_closed());
    }
    let mut c = Client::connect(server.local_addr());
    assert_eq!(c.request(b"GET /ok HTTP/1.1\r\n\r\n").status, 200);
    drop(c);
    wait_until("the connection count to return to zero", || {
        server.open_connections() == 0
    });
    server.shutdown();
}

fn timed() -> ServerConfig {
    ServerConfig {
        request_timeout: Duration::from_millis(300),
        slow_grace: Duration::from_millis(300),
        min_transfer_rate: 64 << 20,
        ..small()
    }
}

/// Sends `bytes` one at a time, `gap` apart, until the server closes;
/// how long it stayed open.
fn trickle(s: &mut TcpStream, bytes: &[u8], gap: Duration) -> Duration {
    let started = Instant::now();
    for b in bytes {
        if s.write_all(&[*b]).is_err() {
            break;
        }
        thread::sleep(gap);
        let _ = s.set_nonblocking(true);
        let mut buf = [0u8; 512];
        let closed = match s.read(&mut buf) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
        };
        let _ = s.set_nonblocking(false);
        if closed {
            break;
        }
    }
    started.elapsed()
}

#[test]
fn a_trickled_head_is_closed_at_the_request_timeout() {
    let server = start(timed(), router());
    let mut s = TcpStream::connect(server.local_addr()).expect("connects");
    let head = [b'G'; 64];
    let open = trickle(&mut s, &head, Duration::from_millis(50));
    assert!(open >= Duration::from_millis(250), "{open:?}");
    assert!(open < Duration::from_millis(2000), "{open:?}");
    server.shutdown();
}

#[test]
fn a_trickled_body_is_closed_at_the_request_timeout() {
    let server = start(timed(), router());
    let mut s = TcpStream::connect(server.local_addr()).expect("connects");
    s.write_all(
        b"POST /count HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: 64\r\n\r\n",
    )
    .expect("sends");
    let open = trickle(&mut s, &[b' '; 64], Duration::from_millis(50));
    assert!(open >= Duration::from_millis(250), "{open:?}");
    assert!(open < Duration::from_millis(2000), "{open:?}");
    server.shutdown();
}

#[test]
fn a_big_bulk_read_slowly_by_the_server_is_not_cut_off() {
    // The handler stands for a busy write path: it waits before reading and
    // between chunks, so the upload takes many times the grace. The client
    // keeps up all along; only this side is slow, and it is not charged.
    let router = Router::new()
        .route(
            Method::Post,
            "/_bulk",
            &[],
            |req: &mut Request<'_>, resp: &mut Response<'_>| -> Result<(), RestError> {
                thread::sleep(Duration::from_millis(500));
                let mut bytes = 0usize;
                while let Some(c) = req.body().next_chunk()? {
                    bytes = bytes.saturating_add(c.len());
                    thread::sleep(Duration::from_millis(10));
                }
                resp.send(200, JSON, json!({"bytes": bytes}).to_string().as_bytes())
            },
        )
        .expect("routes");
    let server = start(timed(), router);
    let body = b"{\"index\":{}}\n{\"a\":1}\n".repeat(24 << 10);
    let started = Instant::now();
    let mut c = Client::connect(server.local_addr());
    let head = format!(
        "POST /_bulk HTTP/1.1\r\ncontent-type: application/x-ndjson\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    c.send(head.as_bytes()).expect("sends");
    let writer = c.writer().try_clone().expect("clones");
    let upload = thread::spawn(move || {
        let mut w = writer;
        w.write_all(&body)
    });
    let r = c.reply().expect("a reply");
    upload.join().expect("joins").expect("uploads");
    assert_eq!(
        (r.status, r.json()),
        (200, json!({"bytes": 21 * (24 << 10)}))
    );
    assert!(
        started.elapsed() > Duration::from_secs(1),
        "slower than the grace"
    );
    server.shutdown();
}

#[test]
fn a_client_that_does_not_read_its_response_is_cut_off() {
    let failed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&failed);
    let router = Router::new()
        .route(
            Method::Get,
            "/big",
            &[],
            move |_: &mut Request<'_>, resp: &mut Response<'_>| -> Result<(), RestError> {
                let mut w = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                let block = [b' '; 4096];
                for _ in 0..(256 << 10) {
                    if let Err(e) = w.write_all(&block) {
                        seen.store(true, Ordering::Release);
                        return Err(e.into());
                    }
                }
                w.finish()
            },
        )
        .expect("routes");
    let server = start(timed(), router);
    let mut s = TcpStream::connect(server.local_addr()).expect("connects");
    s.write_all(b"GET /big HTTP/1.1\r\n\r\n").expect("sends");
    // Reads a little, often: every write makes progress, none is fast.
    let stop = Arc::clone(&failed);
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 16 << 10];
        while !stop.load(Ordering::Acquire) {
            if matches!(s.read(&mut buf), Ok(0) | Err(_)) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    wait_until("the slow write to fail", || failed.load(Ordering::Acquire));
    reader.join().expect("joins");
    server.shutdown();
}

#[test]
fn a_bad_content_type_wins_over_an_unknown_parameter() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(
        b"POST /ok?nope=1 HTTP/1.1\r\ncontent-type: text/csv\r\ncontent-length: 2\r\n\r\n{}",
    );
    assert_eq!(r.status, 406);
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"POST /ok HTTP/1.1\r\ncontent-type: application/json\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}");
    assert_eq!(r.status, 400);
    let e = r.json();
    assert_eq!(e["error"]["type"], "illegal_argument_exception");
    assert_eq!(
        e["error"]["reason"],
        "only one Content-Type header should be provided"
    );
    server.shutdown();
}

#[test]
fn a_uri_java_cannot_parse_is_not_echoed() {
    let server = start(small(), router());
    for target in ["/<script>", "/a%zz", "/a|b", "/a?q=\"x\"", "/a{b}"] {
        let mut c = Client::connect(server.local_addr());
        let raw = format!("GET {target} HTTP/1.1\r\n\r\n");
        let r = c.request(raw.as_bytes());
        assert_eq!(r.status, 400, "{target}");
        assert_eq!(
            r.json(),
            json!({"error": "invalid uri has been requested"}),
            "{target}"
        );
    }
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"GET /a%20b?x=%41 HTTP/1.1\r\n\r\n");
    assert_eq!(
        r.json(),
        json!({"error": "no handler found for uri [/a b] and method [GET]"})
    );
    server.shutdown();
}

#[test]
fn an_http_10_client_gets_a_streamed_body_unframed_then_a_close() {
    let router = Router::new()
        .route(
            Method::Get,
            "/stream",
            &[],
            |_: &mut Request<'_>, resp: &mut Response<'_>| -> Result<(), RestError> {
                let mut w = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                w.write_all(b"[1,")?;
                w.write_all(b"2]")?;
                w.finish()
            },
        )
        .expect("routes");
    let server = start(small(), router);
    let mut s = TcpStream::connect(server.local_addr()).expect("connects");
    s.write_all(b"GET /stream HTTP/1.0\r\nconnection: keep-alive\r\n\r\n")
        .expect("sends");
    let mut raw = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = s.read(&mut buf).expect("reads");
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let all = String::from_utf8(raw).expect("utf-8");
    let (head, body) = all.split_once("\r\n\r\n").expect("a head");
    assert!(!head.contains("transfer-encoding"), "{head}");
    assert!(head.contains("connection: close"), "{head}");
    assert_eq!(body, "[1,2]");
    server.shutdown();
}

#[test]
fn an_echoed_id_too_long_for_the_write_buffer_is_dropped_not_the_reply() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let long = "x".repeat(6000);
    let raw = format!("GET /ok HTTP/1.1\r\nx-opaque-id: {long}\r\nx-request-id: short\r\n\r\n");
    let r = c.request(raw.as_bytes());
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-opaque-id"), None);
    assert_eq!(r.header("x-request-id"), Some("short"));
    server.shutdown();
}
