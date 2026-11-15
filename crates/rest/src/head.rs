//! The request head: request line and headers, parsed in place.
//!
//! The head is read into a per-worker buffer and parsed there: the method,
//! target and every header are ranges into it, so parsing allocates nothing.
//! The size limits are Netty's `HttpObjectDecoder` limits as OpenSearch sets
//! them (`http.max_initial_line_length` 4 KiB, `http.max_header_size`
//! 16 KiB, server/src/main/java/org/opensearch/http/HttpTransportSettings.java
//! at huyz0/OpenSearch@49f238ed), refused with 400 and the type and message of
//! Netty's `TooLongHttpLineException` and `TooLongHttpHeaderException`.
//! Like Netty, empty lines before the request line are skipped and a bare
//! `\n` ends a line.

use std::ops::Range;

use crate::error::{Exception, RestError};

/// The HTTP version of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// `HTTP/1.0`: the connection closes after the response unless the
    /// client asks to keep it alive.
    Http10,
    /// `HTTP/1.1`: the connection stays open unless either side closes it.
    Http11,
}

/// A parsed request head; every field is a range into the buffer it was
/// parsed from.
#[derive(Debug)]
pub(crate) struct Head {
    pub(crate) method: Range<usize>,
    pub(crate) target: Range<usize>,
    pub(crate) version: Version,
    headers: Vec<(Range<usize>, Range<usize>)>,
    max_headers: usize,
}

/// The head's limits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeadLimits {
    pub(crate) max_initial_line: usize,
    pub(crate) max_header_bytes: usize,
}

/// What a scan of the bytes read so far found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scan {
    /// The head is complete and ends before this offset.
    Complete(usize),
    /// More bytes are needed.
    Partial,
}

fn too_long_line(max: usize) -> RestError {
    // ALLOC: error path; a fixed message.
    let msg = format!("An HTTP line is larger than {max} bytes.");
    RestError::exception(400, Exception::new("too_long_http_line_exception", msg)).closing()
}

fn too_long_header(max: usize) -> RestError {
    // ALLOC: error path; a fixed message.
    let msg = format!("HTTP header is larger than {max} bytes.");
    RestError::exception(400, Exception::new("too_long_http_header_exception", msg)).closing()
}

fn bad(reason: &'static str) -> RestError {
    RestError::illegal_argument(reason).closing()
}

/// The length of leading empty lines, which Netty skips.
fn leading_blank(buf: &[u8]) -> usize {
    buf.iter()
        .take_while(|&&b| b == b'\r' || b == b'\n')
        .count()
}

/// Finds the end of the head in `buf`, or refuses a head past the limits.
pub(crate) fn scan(buf: &[u8], limits: &HeadLimits) -> Result<Scan, RestError> {
    let start = leading_blank(buf);
    let rest = &buf[start..];
    let Some(line_end) = rest.iter().position(|&b| b == b'\n') else {
        if rest.len() > limits.max_initial_line {
            return Err(too_long_line(limits.max_initial_line));
        }
        return Ok(Scan::Partial);
    };
    if trim_cr(&rest[..line_end]).len() > limits.max_initial_line {
        return Err(too_long_line(limits.max_initial_line));
    }
    let headers_start = line_end.saturating_add(1);
    let mut pos = headers_start;
    loop {
        let Some(nl) = rest[pos..].iter().position(|&b| b == b'\n') else {
            if rest.len().saturating_sub(headers_start) > limits.max_header_bytes {
                return Err(too_long_header(limits.max_header_bytes));
            }
            return Ok(Scan::Partial);
        };
        let line = trim_cr(&rest[pos..pos.saturating_add(nl)]);
        let next = pos.saturating_add(nl).saturating_add(1);
        if line.is_empty() {
            return Ok(Scan::Complete(start.saturating_add(next)));
        }
        if next.saturating_sub(headers_start) > limits.max_header_bytes {
            return Err(too_long_header(limits.max_header_bytes));
        }
        pos = next;
    }
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn offset(base: usize, r: Range<usize>) -> Range<usize> {
    base.saturating_add(r.start)..base.saturating_add(r.end)
}

impl Head {
    /// Room for `max_headers` headers, allocated once per worker.
    pub(crate) fn new(max_headers: usize) -> Self {
        Self {
            method: 0..0,
            target: 0..0,
            version: Version::Http11,
            // ALLOC: once per worker, `max_headers` pairs, reused per request.
            headers: Vec::with_capacity(max_headers),
            max_headers,
        }
    }

    /// Parses the complete head `buf[..end]` that [`scan`] found.
    pub(crate) fn parse(&mut self, buf: &[u8], end: usize) -> Result<(), RestError> {
        self.headers.clear();
        let start = leading_blank(buf);
        let head = &buf[start..end];
        let line_len = head.iter().position(|&b| b == b'\n').unwrap_or(head.len());
        let line = trim_cr(&head[..line_len]);
        let mut parts = line.split(|&b| b == b' ');
        let (Some(method), Some(target), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad("invalid HTTP request line"));
        };
        if method.is_empty() || !method.iter().all(|&b| is_token(b)) {
            return Err(bad("invalid HTTP method"));
        }
        if target.is_empty() || std::str::from_utf8(target).is_err() {
            return Err(bad("invalid HTTP request target"));
        }
        self.version = match version {
            b"HTTP/1.1" => Version::Http11,
            b"HTTP/1.0" => Version::Http10,
            _ => return Err(bad("unsupported HTTP version")),
        };
        let method_at = 0;
        let target_at = method.len().saturating_add(1);
        self.method = offset(start, method_at..method.len());
        self.target = offset(start, target_at..target_at.saturating_add(target.len()));
        let mut pos = line_len.saturating_add(1);
        while pos < head.len() {
            let nl = head[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .unwrap_or(head.len().saturating_sub(pos));
            let line = trim_cr(&head[pos..pos.saturating_add(nl)]);
            let line_at = pos;
            pos = pos.saturating_add(nl).saturating_add(1);
            if line.is_empty() {
                break;
            }
            if line[0] == b' ' || line[0] == b'\t' {
                return Err(bad("obsolete header line folding is not supported"));
            }
            let Some(colon) = line.iter().position(|&b| b == b':') else {
                return Err(bad("invalid HTTP header"));
            };
            let name = &line[..colon];
            if name.is_empty() || !name.iter().all(|&b| is_token(b)) {
                return Err(bad("invalid HTTP header name"));
            }
            let raw = &line[colon.saturating_add(1)..];
            let lead = raw.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
            let trail = raw[lead..]
                .iter()
                .rev()
                .take_while(|&&b| b == b' ' || b == b'\t')
                .count();
            let value = &raw[lead..raw.len().saturating_sub(trail)];
            if std::str::from_utf8(value).is_err() || value.iter().any(|&b| b < 0x20 && b != b'\t')
            {
                return Err(bad("invalid HTTP header value"));
            }
            if self.headers.len() == self.max_headers {
                return Err(too_many_headers(self.max_headers));
            }
            let value_at = colon.saturating_add(1).saturating_add(lead);
            self.headers.push((
                offset(start.saturating_add(line_at), 0..colon),
                offset(
                    start.saturating_add(line_at),
                    value_at..value_at.saturating_add(value.len()),
                ),
            ));
        }
        Ok(())
    }

    /// The first value of header `name`, compared case-insensitively.
    pub(crate) fn header<'b>(&self, buf: &'b [u8], name: &str) -> Option<&'b str> {
        self.headers(buf, name).next()
    }

    /// Every value of header `name`, in order.
    pub(crate) fn headers<'b, 'n>(
        &'n self,
        buf: &'b [u8],
        name: &'n str,
    ) -> impl Iterator<Item = &'b str> + 'n
    where
        'b: 'n,
    {
        self.headers.iter().filter_map(move |(n, v)| {
            if buf[n.clone()].eq_ignore_ascii_case(name.as_bytes()) {
                std::str::from_utf8(&buf[v.clone()]).ok()
            } else {
                None
            }
        })
    }

    /// How many headers the head has.
    #[cfg(test)]
    pub(crate) fn header_count(&self) -> usize {
        self.headers.len()
    }
}

fn too_many_headers(max: usize) -> RestError {
    // ALLOC: error path; a fixed message.
    let msg = format!("HTTP request has more than {max} headers.");
    RestError::exception(400, Exception::new("too_long_http_header_exception", msg)).closing()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: HeadLimits = HeadLimits {
        max_initial_line: 64,
        max_header_bytes: 64,
    };

    fn reason(e: &RestError) -> (&str, &str) {
        e.as_exception()
            .map(|x| (x.kind(), x.reason()))
            .unwrap_or_default()
    }

    fn parsed(raw: &[u8]) -> Head {
        let Ok(Scan::Complete(end)) = scan(raw, &LIMITS) else {
            panic!("complete head");
        };
        let mut h = Head::new(4);
        h.parse(raw, end).expect("parses");
        h
    }

    #[test]
    fn a_head_parses_into_ranges() {
        let raw = b"\r\nPOST /i/_search?q=x HTTP/1.1\r\nHost: a\r\nContent-Type:  application/json \r\nX-A: 1\nx-a: 2\r\n\r\nbody";
        let h = parsed(raw);
        assert_eq!(&raw[h.method.clone()], b"POST");
        assert_eq!(&raw[h.target.clone()], b"/i/_search?q=x");
        assert_eq!(h.version, Version::Http11);
        assert_eq!(h.header(raw, "content-type"), Some("application/json"));
        assert_eq!(h.header(raw, "HOST"), Some("a"));
        assert_eq!(h.headers(raw, "x-a").collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(h.header(raw, "missing"), None);
        assert_eq!(h.header_count(), 4);
        assert_eq!(scan(raw, &LIMITS).ok(), Some(Scan::Complete(raw.len() - 4)));
    }

    #[test]
    fn partial_heads_ask_for_more() {
        assert_eq!(scan(b"GET / HT", &LIMITS).ok(), Some(Scan::Partial));
        assert_eq!(
            scan(b"GET / HTTP/1.1\r\nHost: a\r\n", &LIMITS).ok(),
            Some(Scan::Partial)
        );
        assert_eq!(scan(b"", &LIMITS).ok(), Some(Scan::Partial));
        let h = parsed(b"GET / HTTP/1.0\n\n");
        assert_eq!(h.version, Version::Http10);
    }

    #[test]
    fn the_initial_line_limit_holds_at_max_and_refuses_one_more() {
        let line = |n: usize| format!("GET /{} HTTP/1.1", "a".repeat(n - 14));
        let at = format!("{}\r\n\r\n", line(64));
        assert!(matches!(
            scan(at.as_bytes(), &LIMITS),
            Ok(Scan::Complete(_))
        ));
        let over = format!("{}\r\n\r\n", line(65));
        let e = scan(over.as_bytes(), &LIMITS).expect_err("one past");
        assert_eq!(
            reason(&e),
            (
                "too_long_http_line_exception",
                "An HTTP line is larger than 64 bytes."
            )
        );
        assert!(e.closes_connection());
        // Without its newline yet, a line already past the limit is refused.
        assert!(scan(line(65).as_bytes(), &LIMITS).is_err());
        assert!(scan(line(64).as_bytes(), &LIMITS).is_ok());
    }

    #[test]
    fn the_header_limit_holds_at_max_and_refuses_one_more() {
        // Each header line counts with its line ending: 62 bytes plus CRLF.
        let at = format!("GET / HTTP/1.1\r\nA: {}\r\n\r\n", "b".repeat(59));
        assert!(matches!(
            scan(at.as_bytes(), &LIMITS),
            Ok(Scan::Complete(_))
        ));
        let over = format!("GET / HTTP/1.1\r\nA: {}\r\n\r\n", "b".repeat(60));
        let e = scan(over.as_bytes(), &LIMITS).expect_err("one past");
        assert_eq!(
            reason(&e),
            (
                "too_long_http_header_exception",
                "HTTP header is larger than 64 bytes."
            )
        );
        let partial = format!("GET / HTTP/1.1\r\nA: {}", "b".repeat(70));
        assert!(scan(partial.as_bytes(), &LIMITS).is_err());
    }

    #[test]
    fn the_header_count_limit_holds_at_max_and_refuses_one_more() {
        let h = parsed(b"GET / HTTP/1.1\r\na:1\r\nb:2\r\nc:3\r\nd:4\r\n\r\n");
        assert_eq!(h.header_count(), 4);
        let raw = b"GET / HTTP/1.1\r\na:1\r\nb:2\r\nc:3\r\nd:4\r\ne:5\r\n\r\n";
        let Ok(Scan::Complete(end)) = scan(raw, &LIMITS) else {
            panic!("complete")
        };
        let e = Head::new(4).parse(raw, end).expect_err("one past");
        assert_eq!(
            reason(&e),
            (
                "too_long_http_header_exception",
                "HTTP request has more than 4 headers."
            )
        );
    }

    #[test]
    fn malformed_heads_are_refused() {
        for raw in [
            &b"GET /\r\n\r\n"[..],
            b"GET / HTTP/1.1 x\r\n\r\n",
            b"G(T / HTTP/1.1\r\n\r\n",
            b" / HTTP/1.1\r\n\r\n",
            b"GET / HTTP/2\r\n\r\n",
            b"GET \xff HTTP/1.1\r\n\r\n",
            b"GET  HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.1\r\nnocolon\r\n\r\n",
            b"GET / HTTP/1.1\r\n: v\r\n\r\n",
            b"GET / HTTP/1.1\r\nb@d: v\r\n\r\n",
            b"GET / HTTP/1.1\r\na: v\r\n folded\r\n\r\n",
            b"GET / HTTP/1.1\r\na: \xff\r\n\r\n",
            b"GET / HTTP/1.1\r\na: x\x01y\r\n\r\n",
        ] {
            let Ok(Scan::Complete(end)) = scan(raw, &LIMITS) else {
                panic!("complete {raw:?}")
            };
            let e = Head::new(4).parse(raw, end).expect_err("malformed");
            assert_eq!(e.status(), 400, "{raw:?}");
            assert!(e.closes_connection());
        }
    }
}
