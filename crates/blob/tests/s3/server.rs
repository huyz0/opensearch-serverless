//! The emulator's S3 front end, driven with raw requests: how it answers
//! what the client never sends.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, SystemTime};

use crate::{BUCKET, REGION, credentials, fixture, put};
use oss_blob::emulator::http::MAX_CONNECTIONS;
use oss_blob::s3::sigv4::{self, Scope, UNSIGNED_PAYLOAD};
use oss_blob::*;

pub(crate) fn connect(endpoint: &str) -> TcpStream {
    let s = TcpStream::connect(endpoint.trim_start_matches("http://")).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s
}

/// Sends `raw` and reads until the server closes or 300 ms pass quietly.
pub(crate) fn exchange(endpoint: &str, raw: &[u8]) -> String {
    let mut s = connect(endpoint);
    s.write_all(raw).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A request signed as the client signs, with `extra` headers signed too.
pub(crate) fn signed(endpoint: &str, method: &str, target: &str, extra: &[(&str, &str)]) -> String {
    let host = endpoint.trim_start_matches("http://");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let date = sigv4::amz_date(now);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = vec![
        ("host", host),
        ("x-amz-content-sha256", UNSIGNED_PAYLOAD),
        ("x-amz-date", date.as_str()),
    ];
    headers.extend_from_slice(extra);
    let pairs: Vec<(&str, &str)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .collect();
    let canonical = sigv4::canonical_request(
        method,
        path,
        &sigv4::canonical_query(&pairs),
        &headers,
        UNSIGNED_PAYLOAD,
    );
    let scope = Scope {
        date: &date[..8],
        region: REGION,
        service: "s3",
    };
    let auth = sigv4::authorization(&credentials(), &scope, &date, &canonical);
    let mut req = format!("{method} {target} HTTP/1.1\r\n");
    for (n, v) in &headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str(&format!("authorization: {auth}\r\n\r\n"));
    req
}

/// Reads until the server closes, failing if the 5 s timeout comes first.
pub(crate) fn read_to_close(s: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf).unwrap();
        if n == 0 {
            return String::from_utf8_lossy(&out).into_owned();
        }
        out.extend_from_slice(&buf[..n]);
    }
}

pub(crate) fn status(response: &str) -> &str {
    response.split(' ').nth(1).unwrap_or("")
}

#[test]
fn malformed_heads_are_refused_and_the_connection_closed() {
    let f = fixture();
    let ep = f.server.endpoint();
    let long = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(9000));
    let many = format!("GET / HTTP/1.1\r\n{}\r\n", "x-h: 1\r\n".repeat(65));
    let cases: [&[u8]; 6] = [
        b"GARBAGE\r\n\r\n",
        b"GET / HTTP/1.0\r\n\r\n",
        long.as_bytes(),
        many.as_bytes(),
        b"GET / HTTP/1.1\r\nno-colon\r\n\r\n",
        b"GET / HTTP/1.1\r\n\xff: x\r\n\r\n",
    ];
    for raw in cases {
        let r = exchange(&ep, raw);
        assert_eq!(status(&r), "400", "{r}");
        assert!(r.contains("connection: close"));
    }
}

#[test]
fn a_head_cut_short_is_refused() {
    let f = fixture();
    for raw in [&b"GET / HTTP/1.1\r\nhost: x"[..], b"GET / HTTP/1.1\r\n"] {
        let mut s = connect(&f.server.endpoint());
        s.write_all(raw).unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap();
        let out = String::from_utf8_lossy(&buf[..n]).into_owned();
        assert_eq!(status(&out), "400", "{out}");
    }
}

#[test]
fn bodies_need_a_declared_length() {
    let f = fixture();
    let ep = f.server.endpoint();
    let chunked = signed(
        &ep,
        "PUT",
        &format!("/{BUCKET}/k"),
        &[("transfer-encoding", "chunked")],
    );
    assert_eq!(status(&exchange(&ep, chunked.as_bytes())), "411");
    let bad = signed(
        &ep,
        "PUT",
        &format!("/{BUCKET}/k"),
        &[("content-length", "abc")],
    );
    assert_eq!(status(&exchange(&ep, bad.as_bytes())), "400");
    let none = signed(&ep, "PUT", &format!("/{BUCKET}/k"), &[]);
    let r = exchange(&ep, none.as_bytes());
    assert_eq!(status(&r), "411", "{r}");
    assert!(r.contains("MissingContentLength"));
    assert_eq!(f.server.store().object_count(), 0);
}

#[test]
fn unsigned_and_badly_signed_requests_are_forbidden() {
    let f = fixture();
    let ep = f.server.endpoint();
    let host = ep.trim_start_matches("http://");
    let cases = [
        (
            format!("GET /{BUCKET}/k HTTP/1.1\r\nhost: {host}\r\n\r\n"),
            "AccessDenied",
        ),
        (
            format!(
                "GET /{BUCKET}/k HTTP/1.1\r\nhost: {host}\r\nx-amz-date: 20261115T000000Z\r\nauthorization: x\r\n\r\n"
            ),
            "InvalidRequest",
        ),
        (
            format!(
                "GET /{BUCKET}/k HTTP/1.1\r\nhost: {host}\r\nx-amz-date: 20261115T000000Z\r\nx-amz-content-sha256: UNSIGNED-PAYLOAD\r\nauthorization: AWS4-HMAC-SHA256 Credential=a, SignedHeaders=x-amz-date, Signature=0\r\n\r\n"
            ),
            "AuthorizationHeaderMalformed",
        ),
        (
            format!(
                "GET /{BUCKET}/k HTTP/1.1\r\nhost: {host}\r\nx-amz-date: 2\r\nx-amz-content-sha256: UNSIGNED-PAYLOAD\r\nauthorization: AWS4-HMAC-SHA256 Credential=a, SignedHeaders=host, Signature=0\r\n\r\n"
            ),
            "AuthorizationHeaderMalformed",
        ),
        (
            format!(
                "GET /{BUCKET}/%zz HTTP/1.1\r\nhost: {host}\r\nx-amz-date: 20261115T000000Z\r\nx-amz-content-sha256: UNSIGNED-PAYLOAD\r\nauthorization: AWS4-HMAC-SHA256 Credential=a, SignedHeaders=host, Signature=0\r\n\r\n"
            ),
            "InvalidURI",
        ),
        (
            format!(
                "GET /{BUCKET}?a=%zz HTTP/1.1\r\nhost: {host}\r\nx-amz-date: 20261115T000000Z\r\nx-amz-content-sha256: UNSIGNED-PAYLOAD\r\nauthorization: AWS4-HMAC-SHA256 Credential=a, SignedHeaders=host, Signature=0\r\n\r\n"
            ),
            "InvalidURI",
        ),
    ];
    for (raw, code) in cases {
        let r = exchange(&ep, raw.as_bytes());
        assert_eq!(status(&r), "403", "{r}");
        assert!(r.contains(code), "{code}: {r}");
    }
}

#[test]
fn signed_requests_the_front_end_does_not_serve() {
    let f = fixture();
    put(f.server.store(), "k", b"abc");
    let ep = f.server.endpoint();
    let cases = [
        ("POST", format!("/{BUCKET}/k"), vec![], "405"),
        ("GET", "/other-bucket/k".to_owned(), vec![], "404"),
        ("GET", format!("/{BUCKET}x/k"), vec![], "404"),
        (
            "GET",
            format!("/{BUCKET}/k"),
            vec![("range", "items=0-1")],
            "400",
        ),
        ("GET", format!("/{BUCKET}"), vec![], "400"),
        (
            "GET",
            format!("/{BUCKET}?list-type=2&max-keys=abc"),
            vec![],
            "400",
        ),
    ];
    for (method, target, extra, want) in cases {
        let raw = signed(&ep, method, &target, &extra);
        let r = exchange(&ep, raw.as_bytes());
        assert_eq!(status(&r), want, "{method} {target}: {r}");
    }
    // A trailing slash on the bucket lists it.
    let raw = signed(&ep, "GET", &format!("/{BUCKET}/?list-type=2"), &[]);
    let r = exchange(&ep, raw.as_bytes());
    assert_eq!(status(&r), "200");
    assert!(
        r.contains("<Key>k</Key>") && r.contains("<KeyCount>1</KeyCount>"),
        "{r}"
    );
}

#[test]
fn a_client_asking_to_close_gets_its_answer_then_eof() {
    let f = fixture();
    put(f.server.store(), "k", b"abc");
    let ep = f.server.endpoint();
    let raw = signed(
        &ep,
        "DELETE",
        &format!("/{BUCKET}/k"),
        &[("connection", "close")],
    );
    let mut s = connect(&ep);
    s.write_all(raw.as_bytes()).unwrap();
    // The read ends at the server's close, well inside the 5 s timeout.
    let r = read_to_close(&mut s);
    assert_eq!(status(&r), "204");
    assert!(r.contains("connection: close"));
    assert_eq!(f.server.store().object_count(), 0);
}

#[test]
fn head_and_delete_failures_reach_the_client() {
    let f = fixture();
    f.server.store().fail_next(S3Op::Head, 4);
    assert_eq!(f.client.head("k"), Err(BlobError::Unavailable));
    f.server.store().throttle_next(S3Op::Delete, 4);
    assert_eq!(f.client.delete("k"), Err(BlobError::Throttled));
    assert_eq!(f.server.requests(), 8);
}

#[test]
fn the_request_log_keeps_the_latest_256() {
    let f = fixture();
    for _ in 0..260 {
        f.client.head("k").unwrap();
    }
    let seen = f.server.seen();
    assert_eq!(seen.len(), 256);
    assert!(seen.iter().all(|s| s.method == "HEAD"));
    assert_eq!(f.server.requests(), 260);
}

#[test]
fn connections_past_the_limit_are_closed_on_arrival() {
    let f = fixture();
    let ep = f.server.endpoint();
    let held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| connect(&ep)).collect();
    let mut extra = connect(&ep);
    let mut buf = [0u8; 1];
    assert_eq!(extra.read(&mut buf).unwrap(), 0);
    drop(held);
    // Once the held connections are gone, new ones are served again.
    let mut served = false;
    for _ in 0..50 {
        if f.client.head("k").is_ok() {
            served = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(served);
}

#[test]
fn answering_zero_requests_clears_the_script() {
    let f = fixture();
    f.server.answer_next(503, "SlowDown", 3);
    f.server.answer_next(503, "SlowDown", 0);
    assert_eq!(f.client.head("k"), Ok(None));
    assert_eq!(f.server.requests(), 1);
}

#[test]
fn a_dropped_server_stops_listening() {
    let f = fixture();
    let addr = f.server.endpoint().trim_start_matches("http://").to_owned();
    assert!(TcpStream::connect(&addr).is_ok());
    drop(f);
    assert!(TcpStream::connect(&addr).is_err());
}

#[test]
fn a_dropped_server_closes_its_idle_connections() {
    let f = fixture();
    let ep = f.server.endpoint();
    // An idle connection, kept open after one answered request.
    let mut held = connect(&ep);
    held.write_all(signed(&ep, "HEAD", &format!("/{BUCKET}/k"), &[]).as_bytes())
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(held.read(&mut byte).unwrap(), 1);
        head.push(byte[0]);
    }
    // Another connection comes and goes; the idle one must stay tracked.
    let mut other = connect(&ep);
    let close = signed(
        &ep,
        "HEAD",
        &format!("/{BUCKET}/k"),
        &[("connection", "close")],
    );
    other.write_all(close.as_bytes()).unwrap();
    read_to_close(&mut other);
    drop(f);
    match held.read(&mut byte) {
        Ok(n) => assert_eq!(n, 0),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset, "{e}"),
    }
}

#[test]
fn a_line_of_exactly_the_bound_is_read() {
    let f = fixture();
    let ep = f.server.endpoint();
    // `GET /` + padding + ` HTTP/1.1\r\n` is MAX_LINE bytes in all.
    let pad = 8 * 1024 - "GET / HTTP/1.1\r\n".len();
    let raw = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(pad));
    // Read, then refused for want of a signature rather than as malformed.
    assert_eq!(status(&exchange(&ep, raw.as_bytes())), "403");
}

#[test]
fn a_body_the_handler_left_is_drained_and_the_connection_kept() {
    let f = fixture();
    let ep = f.server.endpoint();
    let mut s = connect(&ep);
    // A PUT to another bucket is refused before its body is read.
    let refused = signed(&ep, "PUT", "/other-bucket/k", &[("content-length", "5")]);
    s.write_all(refused.as_bytes()).unwrap();
    s.write_all(b"hello").unwrap();
    let close = signed(
        &ep,
        "HEAD",
        &format!("/{BUCKET}/k"),
        &[("connection", "close")],
    );
    s.write_all(close.as_bytes()).unwrap();
    let r = read_to_close(&mut s);
    let statuses: Vec<&str> = r
        .split("HTTP/1.1 ")
        .skip(1)
        .map(|p| p.get(..3).unwrap_or(""))
        .collect();
    assert_eq!(statuses, ["404", "404"], "{r}");
}

#[test]
fn a_body_cut_short_closes_the_connection_after_the_answer() {
    let f = fixture();
    let ep = f.server.endpoint();
    let mut s = connect(&ep);
    let refused = signed(&ep, "PUT", "/other-bucket/k", &[("content-length", "100")]);
    s.write_all(refused.as_bytes()).unwrap();
    s.write_all(b"hello").unwrap();
    s.shutdown(Shutdown::Write).unwrap();
    // The server cannot drain what never comes: it answers and closes.
    let r = read_to_close(&mut s);
    assert_eq!(status(&r), "404", "{r}");
}

#[test]
fn conditional_writes_answer_as_s3_does() {
    let f = fixture();
    let e = f.server.endpoint();
    let target = format!("/{BUCKET}/c");
    // Both conditions on one PUT is a pair S3 does not implement.
    let both = signed(
        &e,
        "PUT",
        &target,
        &[
            ("content-length", "0"),
            ("if-match", "\"x\""),
            ("if-none-match", "*"),
        ],
    );
    assert_eq!(status(&exchange(&e, both.as_bytes())), "501");
    // A stored object's ETag comes back on PUT, HEAD and GET alike.
    let created = exchange(
        &e,
        signed(&e, "PUT", &target, &[("content-length", "0")]).as_bytes(),
    );
    assert_eq!(status(&created), "200");
    let tag = created
        .lines()
        .find_map(|l| l.strip_prefix("etag: "))
        .unwrap()
        .to_owned();
    for method in ["HEAD", "GET"] {
        let r = exchange(&e, signed(&e, method, &target, &[]).as_bytes());
        assert!(r.contains(&format!("etag: {tag}")), "{method}: {r}");
    }
    // A failed conditional PUT, and DELETEs refused or applied by If-Match.
    f.server.store().fail_next(S3Op::Put, 1);
    let failing = signed(
        &e,
        "PUT",
        &target,
        &[("content-length", "0"), ("if-match", &tag)],
    );
    assert_eq!(status(&exchange(&e, failing.as_bytes())), "500");
    let wrong = signed(&e, "DELETE", &target, &[("if-match", "\"other\"")]);
    assert_eq!(status(&exchange(&e, wrong.as_bytes())), "412");
    let stale = signed(
        &e,
        "PUT",
        &target,
        &[("content-length", "0"), ("if-match", "\"other\"")],
    );
    assert_eq!(status(&exchange(&e, stale.as_bytes())), "412");
    // `If-Match` on a key with no object is S3's 404.
    let missing = format!("/{BUCKET}/none");
    let put = signed(
        &e,
        "PUT",
        &missing,
        &[("content-length", "0"), ("if-match", &tag)],
    );
    assert_eq!(status(&exchange(&e, put.as_bytes())), "404");
    let del = signed(&e, "DELETE", &missing, &[("if-match", &tag)]);
    assert_eq!(status(&exchange(&e, del.as_bytes())), "404");
    f.server.store().fail_next(S3Op::Delete, 1);
    let right = signed(&e, "DELETE", &target, &[("if-match", &tag)]);
    assert_eq!(status(&exchange(&e, right.as_bytes())), "500");
    assert_eq!(status(&exchange(&e, right.as_bytes())), "204");
    assert_eq!(f.server.store().object_count(), 0);
}
