//! Bodies, streaming and every bound, at its limit and one past it.

mod support;

use std::io::Write;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_rest::json::{JsonError, Limits, Token, TokenSink, Tokenizer};
use oss_rest::ndjson::{LineSink, Splitter};
use oss_rest::{JSON, Length, Method, Request, Response, RestError, Router, ServerConfig};
use serde_json::json;
use support::{Client, get, post, small, start};

/// Reads the body chunk by chunk and reports what it saw.
fn count(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let declared = req.body().declared_length();
    let (mut bytes, mut chunks, mut max) = (0usize, 0usize, 0usize);
    while let Some(c) = req.body().next_chunk()? {
        bytes = bytes.saturating_add(c.len());
        chunks = chunks.saturating_add(1);
        max = max.max(c.len());
    }
    let body = json!({"bytes": bytes, "chunks": chunks, "max_chunk": max, "declared": declared});
    resp.send(200, JSON, body.to_string().as_bytes())
}

struct Counter(usize);

impl TokenSink for Counter {
    type Error = RestError;
    fn token(&mut self, _: Token<'_>) -> Result<(), RestError> {
        self.0 = self.0.saturating_add(1);
        Ok(())
    }
}

/// Tokenizes the body as one JSON document.
fn tokens(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let mut t = Tokenizer::new(Limits::default());
    let mut sink = Counter(0);
    while let Some(c) = req.body().next_chunk()? {
        t.feed(c, &mut sink)?;
    }
    t.finish(&mut sink)?;
    resp.send(200, JSON, json!({"tokens": sink.0}).to_string().as_bytes())
}

/// A bulk body: lines into a per-line tokenizer.
struct Bulk {
    t: Tokenizer,
    tokens: Counter,
    lines: u64,
}

impl LineSink for Bulk {
    type Error = RestError;
    fn part(&mut self, bytes: &[u8]) -> Result<(), RestError> {
        self.t.feed(bytes, &mut self.tokens)
    }
    fn end_line(&mut self, _: u64) -> Result<(), RestError> {
        self.t.finish(&mut self.tokens)?;
        self.t.reset();
        self.lines = self.lines.saturating_add(1);
        Ok(())
    }
}

impl From<JsonError> for Box<Bulk> {
    fn from(_: JsonError) -> Self {
        unreachable!()
    }
}

fn bulk(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let mut sink = Bulk {
        t: Tokenizer::new(Limits::default()),
        tokens: Counter(0),
        lines: 0,
    };
    let mut split = Splitter::new();
    while let Some(c) = req.body().next_chunk()? {
        split.feed(c, &mut sink)?;
    }
    split.finish()?;
    resp.send(
        200,
        JSON,
        json!({"lines": sink.lines, "tokens": sink.tokens.0})
            .to_string()
            .as_bytes(),
    )
}

/// A chunked response of `n` bytes.
fn stream(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let n: usize = req.param("n").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut w = resp.start(200, Some("text/plain"), Length::Chunked, &[])?;
    for i in 0..n {
        w.write_all(&[letter(i)])?;
    }
    w.finish()
}

/// The `i`th byte of a streamed body.
fn letter(i: usize) -> u8 {
    b"abcdefghijklmnopqrstuvwxyz"[i.checked_rem(26).unwrap_or(0)]
}

fn router() -> Router {
    Router::new()
        .route(Method::Post, "/_count", &[], count)
        .and_then(|r| r.route(Method::Post, "/_json", &[], tokens))
        .and_then(|r| r.route(Method::Post, "/_bulk", &["refresh"], bulk))
        .and_then(|r| r.route(Method::Get, "/_stream", &["n"], stream))
        .and_then(|r| {
            r.route(
                Method::Get,
                "/",
                &[],
                |_: &mut Request<'_>, resp: &mut Response<'_>| resp.send(200, JSON, b"{}"),
            )
        })
        .expect("routes")
}

#[test]
fn a_length_body_streams_in_body_buffer_chunks() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let body = vec![b'x'; 50_000];
    let r = c.request(&post("/_count", "application/json", &body));
    let v = r.json();
    assert_eq!(
        (v["bytes"].as_u64(), v["declared"].as_u64()),
        (Some(50_000), Some(50_000))
    );
    assert!(v["max_chunk"].as_u64() <= Some(4096), "{v}");
    assert!(v["chunks"].as_u64() >= Some(13));
    server.shutdown();
}

#[test]
fn a_chunked_body_and_expect_continue_stream_too() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    c.send(b"POST /_count HTTP/1.1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n")
        .expect("head");
    let cont = c.reply().expect("100 first");
    assert_eq!(cont.status, 100);
    c.send(b"6\r\nhello \r\n5\r\nworld\r\n0\r\n\r\n")
        .expect("body");
    let r = c.reply().expect("final");
    assert_eq!(r.json()["bytes"], 11);
    assert_eq!(r.json()["declared"], serde_json::Value::Null);
    server.shutdown();
}

#[test]
fn json_and_ndjson_bodies_tokenize_while_streaming() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let doc = json!({"query": {"match": {"title": "x".repeat(20_000)}}, "size": 3});
    let r = c.request(&post(
        "/_json",
        "application/json",
        doc.to_string().as_bytes(),
    ));
    // 12 tokens, the 20,000-byte string in two pieces of at most 16 KiB.
    assert_eq!(r.json(), json!({"tokens": 13}));
    let bad = c.request(&post("/_json", "application/json", b"{\"a\":}"));
    assert_eq!(bad.status, 400);
    assert_eq!(bad.json()["error"]["type"], "json_parse_exception");
    let mut nd = Vec::new();
    for i in 0..200 {
        nd.extend_from_slice(
            format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n{{\"n\":{i}}}\r\n").as_bytes(),
        );
    }
    let r = c.request(&post("/_bulk?refresh=true", "application/x-ndjson", &nd));
    assert_eq!(r.json(), json!({"lines": 400, "tokens": 200 * 7 + 200 * 4}));
    let r = c.request(&post("/_bulk", "application/x-ndjson", b"{}\n{}"));
    assert_eq!(
        r.json()["error"]["reason"],
        "The bulk request must be terminated by a newline [\\n]"
    );
    server.shutdown();
}

#[test]
fn a_streamed_response_is_chunked_in_write_buffer_pieces() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/_stream?n=100000"));
    assert_eq!(
        (r.status, r.header("transfer-encoding")),
        (200, Some("chunked"))
    );
    assert_eq!(r.body.len(), 100_000);
    assert!(
        r.body
            .iter()
            .enumerate()
            .all(|(i, &b)| b == b'a' + (i % 26) as u8)
    );
    server.shutdown();
}

#[test]
fn the_content_length_limit_holds_at_max_and_refuses_one_more() {
    let config = ServerConfig {
        max_content_length: 1000,
        ..small()
    };
    let server = start(config, router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&post("/_count", "application/json", &[b'a'; 1000]));
    assert_eq!(r.json()["bytes"], 1000);
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&post("/_count", "application/json", &[b'a'; 1001]));
    assert_eq!((r.status, r.body.len()), (413, 0));
    assert!(c.is_closed());
    // A chunked body is cut off once it passes the limit.
    let mut c = Client::connect(server.local_addr());
    c.send(b"POST /_count HTTP/1.1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n3e8\r\n").expect("head");
    c.send(&[b'a'; 1000]).expect("data");
    c.send(b"\r\n1\r\na\r\n0\r\n\r\n").expect("past");
    let r = c.reply().expect("reply");
    assert_eq!(r.status, 413);
    server.shutdown();
}

#[test]
fn the_initial_line_and_header_limits_hold_at_max_and_refuse_one_more() {
    let config = ServerConfig {
        max_initial_line: 64,
        max_header_bytes: 128,
        ..small()
    };
    let server = start(config, router());
    let line = |n: usize| format!("GET /?{} HTTP/1.1\r\n", "p".repeat(n - 15));
    let mut c = Client::connect(server.local_addr());
    let r = c.request(format!("{}\r\n", line(64)).as_bytes());
    assert_eq!(r.status, 400, "an unknown parameter, but the line was read");
    assert_eq!(r.json()["error"]["type"], "illegal_argument_exception");
    let mut c = Client::connect(server.local_addr());
    let r = c.request(format!("{}\r\n", line(65)).as_bytes());
    assert_eq!(
        r.json()["error"]["reason"],
        "An HTTP line is larger than 64 bytes."
    );
    let header = |n: usize| format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "h".repeat(n - 5));
    let mut c = Client::connect(server.local_addr());
    assert_eq!(c.request(header(128).as_bytes()).status, 200);
    let mut c = Client::connect(server.local_addr());
    let r = c.request(header(129).as_bytes());
    assert_eq!(r.json()["error"]["type"], "too_long_http_header_exception");
    assert!(c.is_closed());
    server.shutdown();
}

#[test]
fn bodies_need_a_supported_content_type() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"POST /_count HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}");
    assert_eq!(
        r.json(),
        json!({"error": "Content-Type header is missing", "status": 406})
    );
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&post("/_count", "application/yaml", b"a: 1"));
    assert_eq!(
        r.json(),
        json!({"error": "Content-Type header [application/yaml] is not supported", "status": 406})
    );
    let mut c = Client::connect(server.local_addr());
    assert_eq!(
        c.request(&post("/_count", "application/json; charset=UTF-8", b"{}"))
            .status,
        200
    );
    server.shutdown();
}

/// A handler that holds its body buffer until released.
fn holding(gate: Arc<Mutex<Receiver<()>>>, entered: SyncSender<()>) -> Router {
    Router::new()
        .route(
            Method::Post,
            "/_hold",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let _ = req.body().next_chunk()?;
                entered.send(()).expect("test listens");
                gate.lock().expect("gate").recv().expect("released");
                while req.body().next_chunk()?.is_some() {}
                resp.send(200, JSON, b"{}")
            },
        )
        .expect("routes")
}

#[test]
fn the_body_buffer_limit_holds_at_max_and_refuses_one_more_with_429() {
    let (release, gate) = sync_channel::<()>(4);
    let (entered_tx, entered) = sync_channel::<()>(4);
    let config = ServerConfig {
        body_buffers: 2,
        body_wait: Duration::from_millis(50),
        ..small()
    };
    let server = start(config, holding(Arc::new(Mutex::new(gate)), entered_tx));
    let mut held: Vec<Client> = (0..2)
        .map(|_| {
            let mut c = Client::connect(server.local_addr());
            c.send(&post("/_hold", "application/json", b"{}"))
                .expect("sends");
            entered
                .recv_timeout(Duration::from_secs(10))
                .expect("handler entered");
            c
        })
        .collect();
    let mut third = Client::connect(server.local_addr());
    let started = Instant::now();
    let r = third.request(&post("/_hold", "application/json", b"{}"));
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "waited before refusing"
    );
    assert_eq!(r.status, 429);
    let reason = "[in_flight_requests] Data too large, data for [<http_request>] would be [12288/12kb], which is larger than the limit of [8192/8kb]";
    assert_eq!(
        r.json(),
        json!({
            "error": {
                "root_cause": [{"type": "circuit_breaking_exception", "reason": reason, "bytes_wanted": 12288, "bytes_limit": 8192, "durability": "TRANSIENT"}],
                "type": "circuit_breaking_exception", "reason": reason, "bytes_wanted": 12288, "bytes_limit": 8192, "durability": "TRANSIENT"
            },
            "status": 429
        })
    );
    for _ in 0..2 {
        release.send(()).expect("release");
    }
    for c in &mut held {
        assert_eq!(c.reply().expect("served").status, 200);
    }
    // With the buffers back, a body is admitted again.
    let mut again = Client::connect(server.local_addr());
    again
        .send(&post("/_hold", "application/json", b"{}"))
        .expect("sends");
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("entered");
    release.send(()).expect("release");
    assert_eq!(again.reply().expect("served").status, 200);
    server.shutdown();
}

#[test]
fn the_connection_limit_holds_at_max_and_refuses_one_more_with_503() {
    let config = ServerConfig {
        workers: 2,
        max_connections: 3,
        ..small()
    };
    let server = start(config, router());
    let mut open: Vec<Client> = (0..3)
        .map(|_| Client::connect(server.local_addr()))
        .collect();
    // Each is accepted (two served, one queued) before the next connects.
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.open_connections() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(server.open_connections(), 3);
    let mut fourth = Client::connect(server.local_addr());
    let r = fourth.reply().expect("refused at once");
    assert_eq!((r.status, r.header("connection")), (503, Some("close")));
    assert!(fourth.is_closed());
    assert_eq!(server.refused_connections(), 1);
    // The ones inside are all served; the queued one once a worker frees.
    for c in &mut open {
        let r = c.request(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert_eq!(r.status, 200);
    }
    server.shutdown();
}

#[test]
fn an_idle_kept_alive_connection_yields_its_worker_to_a_waiting_one() {
    let config = ServerConfig {
        workers: 1,
        max_connections: 2,
        ..small()
    };
    let server = start(config, router());
    let mut idle = Client::connect(server.local_addr());
    assert_eq!(idle.request(&get("/")).status, 200);
    let mut waiting = Client::connect(server.local_addr());
    let started = Instant::now();
    assert_eq!(waiting.request(&get("/")).status, 200);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "served within a few idle polls"
    );
    assert!(idle.is_closed(), "the idle one was closed to make room");
    assert_eq!(server.accepted_connections(), 2);
    server.shutdown();
}

#[test]
fn a_body_the_handler_leaves_unread_is_drained_when_small() {
    let r = Router::new()
        .route(
            Method::Post,
            "/_ignore",
            &[],
            |_: &mut Request<'_>, resp: &mut Response<'_>| resp.send(200, JSON, b"{}"),
        )
        .and_then(|r| {
            r.route(
                Method::Get,
                "/",
                &[],
                |_: &mut Request<'_>, resp: &mut Response<'_>| resp.send(200, JSON, b"{}"),
            )
        })
        .expect("routes");
    let server = start(small(), r);
    let mut c = Client::connect(server.local_addr());
    assert_eq!(
        c.request(&post("/_ignore", "application/json", &[b'a'; 10_000]))
            .status,
        200
    );
    assert_eq!(c.request(&get("/")).status, 200, "still open");
    let mut c = Client::connect(server.local_addr());
    c.send(&post("/_ignore", "application/json", &vec![b'a'; 200_000]))
        .expect("sends");
    assert_eq!(c.reply().expect("reply").status, 200);
    assert!(c.is_closed(), "too much to drain");
    server.shutdown();
}
