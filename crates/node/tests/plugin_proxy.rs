//! docs/specs/plugin-proxy: requests for plugins not ported reach a Java
//! node and its answer comes back, over real sockets with a fake upstream.

// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::plugin_proxy::{PluginProxy, ProxySettings};
use oss_rest::{Method, Request, Response, Router, Server, ServerConfig};

/// One request as the fake upstream saw it.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A fake Java node: answers every request with `reply` and records them.
fn upstream(reply: fn(&Seen) -> Vec<u8>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let log = Arc::clone(&log);
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).expect("header");
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (n, v) = h.split_once(':').expect("header shape");
                    headers.push((n.to_owned(), v.trim().to_owned()));
                }
                let mut seen = Seen {
                    line: line.trim_end().to_owned(),
                    headers,
                    body: Vec::new(),
                };
                if let Some(n) = seen
                    .header("content-length")
                    .and_then(|v| v.parse::<usize>().ok())
                {
                    seen.body = vec![0; n];
                    reader.read_exact(&mut seen.body).expect("body");
                } else if seen.header("transfer-encoding").is_some() {
                    loop {
                        let mut size = String::new();
                        reader.read_line(&mut size).expect("size");
                        let n = usize::from_str_radix(size.trim(), 16).expect("hex");
                        let mut data = vec![0; n + 2];
                        reader.read_exact(&mut data).expect("chunk");
                        if n == 0 {
                            break;
                        }
                        seen.body.extend_from_slice(&data[..n]);
                    }
                }
                let answer = reply(&seen);
                log.lock().expect("log").push(seen);
                let mut out = stream;
                let _ = out.write_all(&answer);
            });
        }
    });
    (base, seen)
}

fn echo(seen: &Seen) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: {}\r\nx-plugin: ism\r\nconnection: close\r\nkeep-alive: timeout=5\r\n\r\n",
        seen.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&seen.body);
    out
}

fn silent(_: &Seen) -> Vec<u8> {
    thread::sleep(Duration::from_secs(5));
    Vec::new()
}

fn start(upstreams: Vec<String>, max: u64, timeout: Duration) -> Server {
    let proxy = Arc::new(
        PluginProxy::new(&ProxySettings {
            upstreams,
            max_request_bytes: max,
            timeout,
        })
        .expect("proxy"),
    );
    let router = Router::new()
        .route(
            Method::Get,
            "/_plugins/_ml/native",
            &[],
            |_: &mut Request<'_>, r: &mut Response<'_>| r.send(200, "text/plain", b"native"),
        )
        .expect("route");
    let router = proxy.register(router).expect("register");
    let budget = budget(256 << 20);
    let config = ServerConfig {
        workers: 4,
        max_connections: 16,
        body_buffer_size: 4096,
        body_buffers: 4,
        write_buffer_size: 4096,
        ..ServerConfig::default()
    };
    Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        config,
        router,
        &budget,
    )
    .expect("starts")
}

/// Sends `raw` and reads the whole answer (the request says `Connection: close`).
fn ask(server: &Server, raw: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut s = TcpStream::connect(server.local_addr()).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    s.write_all(raw).expect("sends");
    let mut all = Vec::new();
    let _ = s.read_to_end(&mut all);
    let split = all
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = String::from_utf8_lossy(&all[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .expect("status line")
        .split(' ')
        .nth(1)
        .expect("code")
        .parse()
        .expect("number");
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let mut body = all[split + 4..].to_vec();
    // A chunked body, decoded.
    if headers_has(&head, "transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    (status, headers, body)
}

fn headers_has(head: &str, needle: &str) -> bool {
    head.to_ascii_lowercase().contains(needle)
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let (mut out, mut rest) = (Vec::new(), raw);
    while let Some(p) = rest.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&rest[..p]).trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&rest[p + 2..p + 2 + n]);
        rest = &rest[p + 2 + n + 2..];
    }
    out
}

fn post(path: &str, body: &[u8], extra: &str) -> Vec<u8> {
    let mut raw = format!(
        "POST {path} HTTP/1.1\r\nHost: client.example\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

fn get(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: client.example\r\nConnection: close\r\n\r\n").into_bytes()
}

fn budget(bytes: usize) -> MemoryBudget {
    let c = BudgetConfig::new(bytes)
        .with_pool(Pool::RequestBodies, bytes)
        .expect("fits");
    MemoryBudget::new(&c)
}

const LONG: Duration = Duration::from_secs(10);

#[test]
fn with_no_upstream_a_plugin_path_is_the_ordinary_400() {
    let router = PluginProxy::new(&ProxySettings {
        upstreams: vec![],
        max_request_bytes: 1,
        timeout: LONG,
    })
    .map(Arc::new)
    .expect("proxy")
    .register(Router::new())
    .expect("register");
    let budget = budget(64 << 20);
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        ServerConfig {
            workers: 2,
            max_connections: 4,
            body_buffer_size: 4096,
            body_buffers: 2,
            write_buffer_size: 4096,
            ..ServerConfig::default()
        },
        router,
        &budget,
    )
    .expect("starts");
    let (status, _, body) = ask(&server, &get("/_plugins/_ism/policies"));
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&body).contains("no handler found"));
    server.shutdown();
}

#[test]
fn a_request_reaches_the_upstream_whole_and_its_answer_comes_back() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base.clone()], 1 << 20, LONG);
    let body = br#"{"policy":{"states":[]}}"#;
    let raw = post(
        "/_plugins/_ism/policies/p1?if_seq_no=3&x=a%20b",
        body,
        "Authorization: Basic YWRtaW46YWRtaW4=\r\nX-Forwarded-For: 10.0.0.9\r\nX-Opaque-Id: o1\r\nKeep-Alive: timeout=1\r\n",
    );
    let (status, headers, answer) = ask(&server, &raw);
    assert_eq!((status, answer.as_slice()), (201, body.as_slice()));
    assert!(headers.contains(&("x-plugin".to_owned(), "ism".to_owned())));
    assert!(headers.contains(&("content-type".to_owned(), "application/json".to_owned())));
    let seen = seen.lock().expect("seen");
    let s = &seen[0];
    assert_eq!(
        s.line,
        "POST /_plugins/_ism/policies/p1?if_seq_no=3&x=a%20b HTTP/1.1"
    );
    assert_eq!(s.body, body);
    assert_eq!(s.header("authorization"), Some("Basic YWRtaW46YWRtaW4="));
    assert_eq!(s.header("host"), base.strip_prefix("http://"));
    assert_eq!(s.header("x-forwarded-host"), Some("client.example"));
    assert_eq!(s.header("x-forwarded-for"), Some("10.0.0.9"));
    assert_eq!(s.header("x-opaque-id"), Some("o1"));
    assert!(
        s.header("keep-alive").is_none(),
        "hop-by-hop is not forwarded"
    );
    server.shutdown();
}

#[test]
fn every_plugin_prefix_is_proxied_and_a_native_route_is_not() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base], 1 << 20, LONG);
    for prefix in oss_node::plugin_proxy::PLUGIN_PREFIXES {
        let (status, ..) = ask(&server, &get(&format!("{prefix}/x")));
        assert_eq!(status, 201, "{prefix}");
    }
    let (status, _, body) = ask(&server, &get("/_plugins/_ml/native"));
    assert_eq!((status, body.as_slice()), (200, b"native".as_slice()));
    assert_eq!(
        seen.lock().expect("seen").len(),
        oss_node::plugin_proxy::PLUGIN_PREFIXES.len()
    );
    let (status, ..) = ask(&server, &get("/_plugins/_mlx/y"));
    assert_eq!(status, 400, "a longer name is not under the prefix");
    server.shutdown();
}

#[test]
fn a_megabyte_body_streams_through() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base], 2 << 20, LONG);
    let body: Vec<u8> = (0..(1 << 20))
        .map(|i| u8::try_from(i % 251).expect("byte"))
        .collect();
    let (status, _, answer) = ask(&server, &post("/_plugins/_ml/models/_upload", &body, ""));
    assert_eq!(status, 201);
    assert_eq!(answer, body);
    assert_eq!(seen.lock().expect("seen")[0].body, body);
    server.shutdown();
}

#[test]
fn a_chunked_request_body_is_reframed_for_the_upstream() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base], 1 << 20, LONG);
    let raw = b"POST /_plugins/_ml/x HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
    let (status, _, answer) = ask(&server, raw);
    assert_eq!((status, answer.as_slice()), (201, b"abcde".as_slice()));
    assert_eq!(seen.lock().expect("seen")[0].body, b"abcde");
    server.shutdown();
}

#[test]
fn a_refused_connect_tries_the_next_upstream_and_all_refused_is_502() {
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").expect("binds");
        format!("http://{}", l.local_addr().expect("addr"))
    };
    let (live, seen) = upstream(echo);
    let server = start(vec![dead.clone(), live], 1 << 20, LONG);
    for _ in 0..3 {
        let (status, ..) = ask(&server, &get("/_plugins/_ml/models"));
        assert_eq!(status, 201);
    }
    assert_eq!(seen.lock().expect("seen").len(), 3);
    server.shutdown();

    let server = start(vec![dead], 1 << 20, LONG);
    let (status, _, body) = ask(&server, &get("/_plugins/_ml/models"));
    assert_eq!(status, 502);
    assert!(String::from_utf8_lossy(&body).contains("plugin_upstream_unavailable"));
    server.shutdown();
}

#[test]
fn a_body_over_the_limit_is_413_and_never_reaches_the_upstream() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base], 1024, LONG);
    let (status, ..) = ask(&server, &post("/_plugins/_ml/x", &vec![b'a'; 4096], ""));
    assert_eq!(status, 413);
    let chunked = b"POST /_plugins/_ml/x HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
    let mut raw = chunked.to_vec();
    for _ in 0..4 {
        raw.extend_from_slice(b"400\r\n");
        raw.extend_from_slice(&[b'a'; 1024]);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");
    let (status, ..) = ask(&server, &raw);
    assert_eq!(status, 413);
    let (status, ..) = ask(&server, &post("/_plugins/_ml/x", &[b'a'; 1024], ""));
    assert_eq!(status, 201, "a body at the limit passes");
    assert_eq!(seen.lock().expect("seen").len(), 1);
    server.shutdown();
}

#[test]
fn a_path_that_could_aim_the_request_elsewhere_is_refused() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base], 1 << 20, LONG);
    for path in [
        "/_plugins/_ml/%2e%2e/_cluster",
        "/_plugins/_ml/a%2Fb",
        "/_plugins/_ml/a%5cb",
        "/_plugins/_ml/../x",
        "/_plugins/_ml//x",
    ] {
        let (status, ..) = ask(&server, &get(path));
        assert_eq!(status, 400, "{path}");
    }
    assert!(seen.lock().expect("seen").is_empty());
    server.shutdown();
}

#[test]
fn an_upstream_that_does_not_answer_is_504() {
    let (base, _) = upstream(silent);
    let server = start(vec![base], 1 << 20, Duration::from_millis(300));
    let (status, _, body) = ask(&server, &get("/_plugins/_alerting/monitors"));
    assert_eq!(status, 504);
    assert!(String::from_utf8_lossy(&body).contains("plugin_upstream_timeout"));
    server.shutdown();
}

/// An upstream that answers with a head promising `promised` bytes, sends `sent` of them, and
/// then goes quiet for five seconds.
fn stalling_upstream(promised: usize, sent: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim_end().is_empty() {
                break;
            }
        }
        let mut out = stream;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {promised}\r\nconnection: close\r\n\r\n"
        );
        let _ = out.write_all(head.as_bytes());
        let _ = out.write_all(&vec![b'x'; sent]);
        thread::sleep(Duration::from_secs(5));
    });
    base
}

// docs/specs/plugin-proxy AC-6: a stall after the head went out is the per-read timeout
#[test]
fn an_upstream_that_stalls_mid_body_ends_the_answer_at_the_timeout() {
    let base = stalling_upstream(1000, 10);
    let server = start(vec![base], 1 << 20, Duration::from_millis(300));
    let at = std::time::Instant::now();
    let mut s = TcpStream::connect(server.local_addr()).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    s.write_all(&get("/_plugins/_alerting/monitors"))
        .expect("sends");
    let mut all = Vec::new();
    let _ = s.read_to_end(&mut all);
    let took = at.elapsed();
    assert!(
        took < Duration::from_secs(4),
        "the proxy waited {took:?} on a stalled upstream"
    );
    let text = String::from_utf8_lossy(&all).into_owned();
    let body_len = text.split("\r\n\r\n").nth(1).map_or(0, str::len);
    assert!(
        body_len < 1000,
        "the answer was cut short, not completed: {body_len} of 1000 bytes in {text:?}"
    );
    server.shutdown();
}

#[test]
fn head_has_no_body_and_options_is_forwarded() {
    let (base, seen) = upstream(echo);
    let server = start(vec![base], 1 << 20, LONG);
    let (status, _, body) = ask(
        &server,
        b"HEAD /_plugins/_ism/policies HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    );
    assert_eq!((status, body.len()), (201, 0));
    let (status, ..) = ask(
        &server,
        b"OPTIONS /_plugins/_ism/policies HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(status, 201);
    let lines: Vec<_> = seen
        .lock()
        .expect("seen")
        .iter()
        .map(|s| s.line.clone())
        .collect();
    assert!(
        lines[0].starts_with("HEAD ") && lines[1].starts_with("OPTIONS "),
        "{lines:?}"
    );
    server.shutdown();
}

#[test]
fn an_upstream_that_is_not_an_origin_is_refused() {
    for bad in [
        "ftp://h",
        "http://",
        "http://u:p@h",
        "http://h/path",
        "h:1",
        "http://h?x=1",
    ] {
        let e = PluginProxy::new(&ProxySettings {
            upstreams: vec![bad.to_owned()],
            max_request_bytes: 1,
            timeout: LONG,
        });
        assert!(e.is_err(), "{bad}");
    }
}
