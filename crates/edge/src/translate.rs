//! HTTP/2 request to HTTP/1.1 head: the checks and the rewrite.

use bytes::{BufMut, BytesMut};
use http::header::{
    CONNECTION, CONTENT_LENGTH, COOKIE, EXPECT, HOST, TE, TRANSFER_ENCODING, UPGRADE,
};
use http::{HeaderMap, Method};

/// Why a request is refused before it reaches the HTTP/1.1 server.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The request is malformed: 400.
    Malformed(&'static str),
    /// The rewritten head is over the HTTP/1.1 limits: 431.
    TooLarge,
}

/// The request target, if it is acceptable origin-form (or `*` for `OPTIONS`).
pub fn valid_target(target: &str, method: &Method) -> bool {
    if target == "*" {
        return *method == Method::OPTIONS;
    }
    target.starts_with('/') && target.bytes().all(|b| b > 0x20 && b < 0x7f && b != b'#')
}

/// The declared body length: `Ok(None)` when absent, an error when repeated or not a number.
pub fn declared_length(headers: &HeaderMap) -> Result<Option<u64>, Refusal> {
    let mut values = headers.get_all(CONTENT_LENGTH).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Refusal::Malformed("more than one content-length"));
    }
    first
        .to_str()
        .ok()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse().ok())
        .map(Some)
        .ok_or(Refusal::Malformed("content-length is not a number"))
}

/// Everything the head needs from a request.
pub struct Head<'a> {
    /// The method.
    pub method: &'a Method,
    /// The path and query.
    pub target: &'a str,
    /// The `:authority`, which wins over `host`.
    pub authority: Option<&'a str>,
    /// The request headers.
    pub headers: &'a HeaderMap,
    /// The declared length, if any.
    pub length: Option<u64>,
    /// Whether the stream ends with the headers.
    pub end_of_stream: bool,
}

/// Writes the HTTP/1.1 head for `head`; `Connection: close`, so a stream is one connection.
///
/// # Errors
///
/// [`Refusal`] when the request is malformed or the head is over the limits.
pub fn write_head(
    head: &Head<'_>,
    max_line: usize,
    max_headers: usize,
) -> Result<BytesMut, Refusal> {
    if !valid_target(head.target, head.method) {
        return Err(Refusal::Malformed("invalid request target"));
    }
    if let Some(te) = head.headers.get(TE)
        && te.as_bytes() != b"trailers"
    {
        return Err(Refusal::Malformed("te other than trailers"));
    }
    // ALLOC: one head per stream, at most the HTTP/1.1 limits (checked below).
    let mut out = BytesMut::with_capacity(512);
    out.put_slice(head.method.as_str().as_bytes());
    out.put_u8(b' ');
    out.put_slice(head.target.as_bytes());
    out.put_slice(b" HTTP/1.1\r\n");
    if out.len() > max_line.saturating_add(2) {
        return Err(Refusal::TooLarge);
    }
    let after_line = out.len();
    let host = head
        .authority
        .or_else(|| head.headers.get(HOST).and_then(|h| h.to_str().ok()));
    if let Some(host) = host {
        out.put_slice(b"host: ");
        out.put_slice(host.as_bytes());
        out.put_slice(b"\r\n");
    }
    out.put_slice(b"connection: close\r\n");
    for (name, value) in head.headers {
        if name == HOST
            || name == CONNECTION
            || name == CONTENT_LENGTH
            || name == TRANSFER_ENCODING
            || name == UPGRADE
            || name == TE
            || name == EXPECT
            || name == COOKIE
            || name.as_str() == "keep-alive"
            || name.as_str() == "proxy-connection"
        {
            continue;
        }
        out.put_slice(name.as_str().as_bytes());
        out.put_slice(b": ");
        out.put_slice(value.as_bytes());
        out.put_slice(b"\r\n");
    }
    let mut cookies = head.headers.get_all(COOKIE).iter();
    if let Some(first) = cookies.next() {
        out.put_slice(b"cookie: ");
        out.put_slice(first.as_bytes());
        for next in cookies {
            out.put_slice(b"; ");
            out.put_slice(next.as_bytes());
        }
        out.put_slice(b"\r\n");
    }
    match head.length {
        // ALLOC: one length line per head, a few bytes.
        Some(n) => out.put_slice(format!("content-length: {n}\r\n").as_bytes()),
        None if head.end_of_stream => {
            if matches!(*head.method, Method::POST | Method::PUT | Method::PATCH) {
                out.put_slice(b"content-length: 0\r\n");
            }
        }
        None => out.put_slice(b"transfer-encoding: chunked\r\n"),
    }
    out.put_slice(b"\r\n");
    if out.len().saturating_sub(after_line) > max_headers {
        return Err(Refusal::TooLarge);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderValue, header::HeaderName};

    fn head<'a>(m: &'a Method, headers: &'a HeaderMap, end: bool) -> Head<'a> {
        Head {
            method: m,
            target: "/a?b=c",
            authority: Some("example:9200"),
            headers,
            length: declared_length(headers).unwrap_or(None),
            end_of_stream: end,
        }
    }

    fn text(b: &BytesMut) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    #[test]
    fn targets_are_origin_form_only() {
        let get = Method::GET;
        for good in ["/", "/a/b?c=d%20e"] {
            assert!(valid_target(good, &get), "{good}");
        }
        for bad in [
            "",
            "a",
            "/a b",
            "/a#f",
            "/a\u{7f}",
            "/é",
            "/a\r\nx",
            "http://x/",
            "*",
        ] {
            assert!(!valid_target(bad, &get), "{bad:?}");
        }
        assert!(valid_target("*", &Method::OPTIONS));
    }

    #[test]
    fn authority_wins_and_connection_headers_go() {
        let mut h = HeaderMap::new();
        h.insert(HOST, HeaderValue::from_static("other"));
        h.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
        h.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        h.insert(EXPECT, HeaderValue::from_static("100-continue"));
        h.insert(
            HeaderName::from_static("x-opaque-id"),
            HeaderValue::from_static("7"),
        );
        let m = Method::GET;
        let out = text(&write_head(&head(&m, &h, true), 4096, 16384).expect("head"));
        assert!(
            out.starts_with("GET /a?b=c HTTP/1.1\r\nhost: example:9200\r\nconnection: close\r\n")
        );
        assert!(out.contains("x-opaque-id: 7\r\n"));
        assert!(
            !out.contains("other")
                && !out.contains("keep-alive")
                && !out.contains("chunked")
                && !out.contains("100-continue")
        );
    }

    #[test]
    fn cookies_are_joined_and_bodies_are_framed() {
        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_static("a=1"));
        h.append(COOKIE, HeaderValue::from_static("b=2"));
        let m = Method::POST;
        let out = text(&write_head(&head(&m, &h, false), 4096, 16384).expect("head"));
        assert!(out.contains("cookie: a=1; b=2\r\n"));
        assert!(out.contains("transfer-encoding: chunked\r\n"));
        let out = text(&write_head(&head(&m, &HeaderMap::new(), true), 4096, 16384).expect("head"));
        assert!(out.contains("content-length: 0\r\n"));
        let mut h = HeaderMap::new();
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("12"));
        let out = text(&write_head(&head(&m, &h, false), 4096, 16384).expect("head"));
        assert!(out.contains("content-length: 12\r\n") && !out.contains("chunked"));
    }

    #[test]
    fn refusals() {
        let m = Method::GET;
        let mut h = HeaderMap::new();
        h.insert(TE, HeaderValue::from_static("gzip"));
        assert_eq!(
            write_head(&head(&m, &h, true), 4096, 16384).unwrap_err(),
            Refusal::Malformed("te other than trailers")
        );
        let mut h = HeaderMap::new();
        h.append(CONTENT_LENGTH, HeaderValue::from_static("1"));
        h.append(CONTENT_LENGTH, HeaderValue::from_static("1"));
        assert!(declared_length(&h).is_err());
        let mut h = HeaderMap::new();
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("+1"));
        assert!(declared_length(&h).is_err());
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static("x-big"),
            HeaderValue::from_str(&"v".repeat(200)).expect("value"),
        );
        assert_eq!(
            write_head(&head(&m, &h, true), 4096, 100).unwrap_err(),
            Refusal::TooLarge
        );
        let none = HeaderMap::new();
        let target = format!("/{}", "a".repeat(5000));
        let long = Head {
            target: &target,
            ..head(&m, &none, true)
        };
        assert_eq!(
            write_head(&long, 4096, 16384).unwrap_err(),
            Refusal::TooLarge
        );
    }
}
