//! A real HTTP/1.1 server with a few routes, and clients for it.

// ARITH: corpus offsets in a test stub are small; an overflow panics the test, which fails it.
#![allow(dead_code, missing_docs, clippy::arithmetic_side_effects)]

use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_rest::{JSON, Length, Method, Request, Response, Router, Server, ServerConfig};

pub mod quic;

pub const WORKERS: usize = 16;
/// The largest body the test server accepts.
pub const MAX_BODY: u64 = 1 << 20;

/// Handlers' shared state.
#[derive(Default)]
pub struct State {
    pub release: AtomicBool,
    pub holding: AtomicUsize,
    /// `/_cluster/health` answers 503 while set.
    pub unhealthy: AtomicBool,
    /// `/_cluster/health` waits for `release` while set.
    pub hold_health: AtomicBool,
    /// The target (path and query) and body of each `_bulk` call, oldest first.
    pub bulks: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    /// `_bulk` answers this status and an error body while not 0.
    pub bulk_status: AtomicUsize,
    /// What `_bulk` answers with on 200, when not empty.
    pub bulk_answer: std::sync::Mutex<String>,
    /// The method, target and body of each search or point-in-time call, oldest first.
    pub searches: std::sync::Mutex<Vec<(String, String, Vec<u8>)>>,
    /// What those routes answer with.
    pub search_answer: std::sync::Mutex<String>,
    /// With a count above 0, the search route answers from a corpus of that many hits sorted
    /// `[1]`, `[2]` and so on, paged by `size` and `search_after`.
    pub corpus: AtomicUsize,
    /// Characters of padding in each corpus hit's source.
    pub padding: AtomicUsize,
    /// The hit after this number sorts equal to it, so a page ending there has a tie.
    pub tie_at: AtomicUsize,
    /// Milliseconds the search route waits before answering.
    pub search_delay_ms: AtomicUsize,
}

/// One page of the corpus the search route serves when `State::corpus` is set.
fn corpus_page(state: &State, body: &[u8]) -> String {
    let body: serde_json::Value = serde_json::from_slice(body).expect("a json body");
    let size = usize::try_from(body["size"].as_u64().expect("a size")).expect("fits");
    let after = body["search_after"][0]
        .as_u64()
        .map_or(0, |n| usize::try_from(n).expect("fits"));
    let total = state.corpus.load(Ordering::Acquire);
    let pad = "x".repeat(state.padding.load(Ordering::Acquire));
    let tie = state.tie_at.load(Ordering::Acquire);
    let hits: Vec<String> = (after + 1..=total.min(after + size))
        .map(|i| {
            let sort = if tie != 0 && i == tie + 1 { tie } else { i };
            format!(r#"{{"_index":"a","_id":"{i}","_score":null,"_source":{{"pad":"{pad}"}},"sort":[{sort}]}}"#)
        })
        .collect();
    format!(
        r#"{{"took":1,"timed_out":false,"_shards":{{"total":1,"successful":1,"failed":0}},"hits":{{"total":{{"value":{total},"relation":"eq"}},"max_score":null,"hits":[{}]}}}}"#,
        hits.join(",")
    )
}

pub fn budget(bytes: usize) -> MemoryBudget {
    let config = BudgetConfig::new(bytes)
        .with_pool(Pool::RequestBodies, bytes)
        .expect("fits");
    MemoryBudget::new(&config)
}

/// Routes: `/` and `/echo` answer JSON, `/sum` counts a body, `/big` streams 8 MiB chunked,
/// `/hold` waits for `release`.
pub fn start(state: &Arc<State>) -> (Server, MemoryBudget, SocketAddr) {
    start_with(state, MAX_BODY)
}

/// As [`start`], with the node's `max_content_length` set.
pub fn start_with(state: &Arc<State>, max_body: u64) -> (Server, MemoryBudget, SocketAddr) {
    let budget = budget(64 << 20);
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let s = Arc::clone(state);
    let hs = Arc::clone(state);
    let bs = Arc::clone(state);
    let ss = Arc::clone(state);
    let router = Router::new()
        .route(
            Method::Get,
            "/",
            &[],
            |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let host = req.header("host").unwrap_or("-").to_owned();
                let xff = req.header("x-forwarded-for").unwrap_or("-").to_owned();
                resp.send(
                    200,
                    JSON,
                    format!("{{\"host\":\"{host}\",\"xff\":\"{xff}\"}}").as_bytes(),
                )
            },
        )
        .and_then(|r| {
            r.route(
                Method::Post,
                "/sum",
                &[],
                |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    let mut total = 0usize;
                    while let Some(chunk) = req.body().next_chunk()? {
                        total = total.saturating_add(chunk.len());
                    }
                    resp.send(200, JSON, format!("{{\"bytes\":{total}}}").as_bytes())
                },
            )
        })
        .and_then(|r| {
            r.route(
                Method::Get,
                "/big",
                &[],
                |_: &mut Request<'_>, resp: &mut Response<'_>| {
                    let mut w = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                    let block = [b'x'; 4096];
                    for _ in 0..2048 {
                        w.write_all(&block)?;
                    }
                    w.finish()
                },
            )
        })
        .and_then(|r| {
            r.route(
                Method::Get,
                "/hold",
                &[],
                move |_: &mut Request<'_>, resp: &mut Response<'_>| {
                    s.holding.fetch_add(1, Ordering::AcqRel);
                    while !s.release.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    s.holding.fetch_sub(1, Ordering::AcqRel);
                    resp.send(200, JSON, b"{}")
                },
            )
        })
        .and_then(|r| {
            r.route(
                Method::Post,
                "/_bulk",
                &["routing", "pipeline", "timeout", "refresh", "require_alias", "_source"],
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    let target = format!("{}?{}", req.raw_path(), req.raw_query());
                    let mut body = Vec::new();
                    while let Some(chunk) = req.body().next_chunk()? {
                        body.extend_from_slice(chunk);
                    }
                    bs.bulks.lock().expect("lock").push((target, body));
                    let status = bs.bulk_status.load(Ordering::Acquire);
                    if status != 0 {
                        let status = u16::try_from(status).expect("status");
                        return resp.send(
                            status,
                            JSON,
                            b"{\"error\":{\"type\":\"circuit_breaking_exception\",\"reason\":\"too big\"},\"status\":429}",
                        );
                    }
                    let answer = bs.bulk_answer.lock().expect("lock").clone();
                    resp.send(200, JSON, answer.as_bytes())
                },
            )
        })
        .and_then(|r| {
            let record = move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let method = format!("{:?}", req.method());
                let target = format!("{}?{}", req.raw_path(), req.raw_query());
                let mut body = Vec::new();
                while let Some(chunk) = req.body().next_chunk()? {
                    body.extend_from_slice(chunk);
                }
                ss.searches.lock().expect("lock").push((method, target, body));
                let delay = ss.search_delay_ms.load(Ordering::Acquire);
                if delay != 0 {
                    std::thread::sleep(Duration::from_millis(delay as u64));
                }
                let answer = if ss.corpus.load(Ordering::Acquire) != 0 {
                    let last = ss.searches.lock().expect("lock").last().expect("recorded").2.clone();
                    corpus_page(&ss, &last)
                } else {
                    ss.search_answer.lock().expect("lock").clone()
                };
                resp.send(200, JSON, answer.as_bytes())
            };
            let record = Arc::new(record);
            let (a, b, c) = (Arc::clone(&record), Arc::clone(&record), record);
            r.route(Method::Post, "/{index}/_search", &["_source", "_source_includes", "_source_excludes"], move |q: &mut Request<'_>, p: &mut Response<'_>| a(q, p))
                .and_then(|r| r.route(Method::Post, "/{index}/_search/point_in_time", &["keep_alive"], move |q: &mut Request<'_>, p: &mut Response<'_>| b(q, p)))
                .and_then(|r| r.route(Method::Delete, "/_search/point_in_time", &[], move |q: &mut Request<'_>, p: &mut Response<'_>| c(q, p)))
        })
        .and_then(|r| {
            let h = Arc::clone(&hs);
            r.route(
                Method::Get,
                "/_cluster/health",
                &[],
                move |_: &mut Request<'_>, resp: &mut Response<'_>| {
                    while h.hold_health.load(Ordering::Acquire)
                        && !h.release.load(Ordering::Acquire)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    if h.unhealthy.load(Ordering::Acquire) {
                        resp.send(503, JSON, b"{\"status\":\"red\"}")
                    } else {
                        resp.send(200, JSON, b"{\"status\":\"green\"}")
                    }
                },
            )
        })
        .expect("routes");
    let config = ServerConfig {
        workers: WORKERS,
        max_connections: 64,
        body_buffers: 16,
        max_content_length: max_body,
        ..ServerConfig::default()
    };
    let server = Server::start(listener, config, router, &budget).expect("starts");
    let addr = server.local_addr();
    (server, budget, addr)
}

/// What one request answered.
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// An HTTP/1.1 request on its own connection.
pub fn h1(addr: SocketAddr, method: &str, path: &str, host: &str, body: &[u8]) -> Answer {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(addr).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {host}\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).expect("head");
    s.write_all(body).expect("body");
    let mut raw = Vec::new();
    let mut piece = [0u8; 8192];
    loop {
        match s.read(&mut piece) {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&piece[..n]),
        }
    }
    parse_h1(&raw)
}

fn parse_h1(raw: &[u8]) -> Answer {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("head end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head
        .split(' ')
        .nth(1)
        .expect("status")
        .parse()
        .expect("number");
    let lower = head.to_ascii_lowercase();
    let content_type = lower
        .lines()
        .find_map(|l| l.strip_prefix("content-type: "))
        .map(str::to_owned);
    let rest = &raw[split.saturating_add(4)..];
    let body = if lower.contains("transfer-encoding: chunked") {
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let eol = rest[at..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .expect("size line")
                .saturating_add(at);
            let n = usize::from_str_radix(std::str::from_utf8(&rest[at..eol]).expect("utf8"), 16)
                .expect("hex");
            if n == 0 {
                break;
            }
            let from = eol.saturating_add(2);
            let to = from.saturating_add(n);
            out.extend_from_slice(&rest[from..to]);
            at = to.saturating_add(2);
        }
        out
    } else {
        rest.to_vec()
    };
    Answer {
        status,
        content_type,
        body,
    }
}

/// An HTTP/2 request over an established client connection.
pub async fn h2_request(
    send: &mut h2::client::SendRequest<Bytes>,
    method: &str,
    uri: &str,
    body: Option<Bytes>,
) -> Result<Answer, h2::Error> {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(())
        .expect("request");
    let (response, mut stream) = send.send_request(request, body.is_none())?;
    if let Some(body) = body {
        stream.send_data(body, true)?;
    }
    let response = response.await?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().expect("text").to_ascii_lowercase());
    let mut recv = response.into_body();
    let mut out = Vec::new();
    while let Some(next) = recv.data().await {
        let data = next?;
        recv.flow_control().release_capacity(data.len())?;
        out.extend_from_slice(&data);
    }
    Ok(Answer {
        status,
        content_type,
        body: out,
    })
}

/// A clear-text HTTP/2 client connection.
pub async fn h2c(addr: SocketAddr) -> h2::client::SendRequest<Bytes> {
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connects");
    let (send, conn) = h2::client::handshake(tcp).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    send
}

/// One gRPC call over an established clear-text HTTP/2 connection.
#[derive(Debug)]
pub struct Reply {
    pub http_status: u16,
    pub content_type: Option<String>,
    /// `grpc-status`, from the headers (Trailers-Only) or the trailers.
    pub grpc_status: Option<u8>,
    pub grpc_message: Option<String>,
    /// The messages, prefix removed.
    pub messages: Vec<Vec<u8>>,
}

/// The wire form of one message with the given compression flag.
pub fn grpc_frame(flag: u8, message: &[u8]) -> Bytes {
    let mut out = vec![flag];
    out.extend_from_slice(&u32::try_from(message.len()).expect("fits").to_be_bytes());
    out.extend_from_slice(message);
    Bytes::from(out)
}

/// Sends `frames` as the request body to `path` and reads the whole reply.
pub async fn grpc_call(
    send: &mut h2::client::SendRequest<Bytes>,
    path: &str,
    extra: &[(&str, &str)],
    frames: Bytes,
) -> Result<Reply, h2::Error> {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("http://node{path}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers");
    for (k, v) in extra {
        builder = builder.header(*k, *v);
    }
    let request = builder.body(()).expect("request");
    let (response, mut stream) = send.send_request(request, false)?;
    stream.send_data(frames, true)?;
    let response = response.await?;
    let http_status = response.status().as_u16();
    let header =
        |h: &http::HeaderMap, name: &str| h.get(name).map(|v| v.to_str().expect("text").to_owned());
    let content_type = header(response.headers(), "content-type");
    let mut grpc_status = header(response.headers(), "grpc-status").map(|v| v.parse().expect("n"));
    let mut grpc_message = header(response.headers(), "grpc-message");
    let mut recv = response.into_body();
    let mut raw = Vec::new();
    while let Some(next) = recv.data().await {
        let data = next?;
        recv.flow_control().release_capacity(data.len())?;
        raw.extend_from_slice(&data);
    }
    if let Some(t) = recv.trailers().await? {
        grpc_status = header(&t, "grpc-status").map(|v| v.parse().expect("n"));
        grpc_message = header(&t, "grpc-message");
    }
    let mut messages = Vec::new();
    let mut at = 0usize;
    while at < raw.len() {
        let size = at.saturating_add(1)..at.saturating_add(5);
        let n = u32::from_be_bytes(raw[size].try_into().expect("four")) as usize;
        let from = at.saturating_add(5);
        let to = from.saturating_add(n);
        messages.push(raw[from..to].to_vec());
        at = to;
    }
    Ok(Reply {
        http_status,
        content_type,
        grpc_status,
        grpc_message,
        messages,
    })
}
