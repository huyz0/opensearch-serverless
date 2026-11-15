//! HTTP/1.1 framing for the emulator's S3 front end: reading a request's
//! head and body off a socket, and writing responses.
//!
//! Only what the S3 client sends is understood: a request line, at most
//! [`MAX_HEADERS`] headers of at most [`MAX_LINE`] bytes each, and a body
//! framed by `Content-Length`. A request with chunked encoding is refused,
//! because S3 needs a declared length.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use crate::chunk::ChunkSource;
use crate::error::{BlobError, BlobResult};

/// The longest request or header line accepted, in bytes: an encoded key
/// of 1,024 bytes is at most 3,072 of them.
pub(crate) const MAX_LINE: usize = 8 * 1024;

/// The most headers one request may carry.
pub(crate) const MAX_HEADERS: usize = 64;

/// A request's head, as it arrived.
#[derive(Debug, Default)]
pub(crate) struct Head {
    pub(crate) method: String,
    /// The path, still percent-encoded.
    pub(crate) path: String,
    /// The query, still percent-encoded, without the `?`.
    pub(crate) query: String,
    /// Names lowercased, values as sent, in the order sent.
    pub(crate) headers: Vec<(String, String)>,
}

impl Head {
    /// The first value of header `name` (lowercase).
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.trim())
    }
}

/// Why a request could not be read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadError {
    /// The client closed the connection between requests.
    Closed,
    /// The head broke a bound or was not HTTP/1.1; answer 400 and close.
    Malformed(&'static str),
}

/// Reads one line, without its `\r\n`, into `line`; at most [`MAX_LINE`].
fn read_line(reader: &mut BufReader<TcpStream>, line: &mut Vec<u8>) -> Result<(), ReadError> {
    line.clear();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|_| ReadError::Malformed("read failed"))?;
        if available.is_empty() {
            return Err(if line.is_empty() {
                ReadError::Closed
            } else {
                ReadError::Malformed("connection closed mid-line")
            });
        }
        let (take, done) = match available.iter().position(|b| *b == b'\n') {
            Some(at) => (at.saturating_add(1), true),
            None => (available.len(), false),
        };
        if line.len().saturating_add(take) > MAX_LINE {
            return Err(ReadError::Malformed("line longer than its bound"));
        }
        line.extend_from_slice(available.get(..take).unwrap_or_default());
        reader.consume(take);
        if done {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(());
        }
    }
}

/// Reads a request head. `line` is the connection's line buffer.
pub(crate) fn read_head(
    reader: &mut BufReader<TcpStream>,
    line: &mut Vec<u8>,
) -> Result<Head, ReadError> {
    read_line(reader, line)?;
    let text = std::str::from_utf8(line).map_err(|_| ReadError::Malformed("not UTF-8"))?;
    let mut parts = text.split(' ');
    let (Some(method), Some(target), Some("HTTP/1.1"), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Malformed("not an HTTP/1.1 request line"));
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut head = Head {
        // ALLOC: one method, inside the MAX_LINE request line.
        method: method.to_owned(),
        // ALLOC: one path, inside the MAX_LINE request line.
        path: path.to_owned(),
        // ALLOC: one query, inside the MAX_LINE request line.
        query: query.to_owned(),
        // ALLOC: at most MAX_HEADERS headers.
        headers: Vec::with_capacity(16),
    };
    loop {
        read_line(reader, line).map_err(|e| match e {
            ReadError::Closed => ReadError::Malformed("connection closed in the head"),
            other => other,
        })?;
        if line.is_empty() {
            return Ok(head);
        }
        if head.headers.len() >= MAX_HEADERS {
            return Err(ReadError::Malformed("too many headers"));
        }
        let text = std::str::from_utf8(line).map_err(|_| ReadError::Malformed("not UTF-8"))?;
        let (name, value) = text
            .split_once(':')
            .ok_or(ReadError::Malformed("header without a colon"))?;
        // ALLOC: one header, at most MAX_LINE bytes, at most MAX_HEADERS of them.
        head.headers
            .push((name.trim().to_ascii_lowercase(), value.to_owned()));
    }
}

/// Percent-decodes `text` into a string, as S3 reads a path or query; a
/// bad escape or a result that is not UTF-8 is `None`.
pub(crate) fn percent_decode(text: &str) -> Option<String> {
    // ALLOC: at most the input's length.
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = char::from(bytes.next()?).to_digit(16)?;
            let lo = char::from(bytes.next()?).to_digit(16)?;
            out.push(u8::try_from(hi.checked_mul(16)?.checked_add(lo)?).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// A request body read straight off the socket, `len` bytes long.
pub(crate) struct SocketBody<'a> {
    pub(crate) reader: &'a mut BufReader<TcpStream>,
    pub(crate) len: u64,
    pub(crate) left: u64,
}

impl SocketBody<'_> {
    /// Reads and drops what the handler left of the body, so the next
    /// request starts where it should. False if the client went away.
    pub(crate) fn drain(&mut self) -> bool {
        let mut scratch = [0u8; 4096];
        while self.left > 0 {
            match self.fill(&mut scratch) {
                Ok(n) if n > 0 => {}
                _ => return false,
            }
        }
        true
    }
}

impl ChunkSource for SocketBody<'_> {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let want = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        // An empty read would still wait on the socket for the next request.
        let Some(out) = buf.get_mut(..want).filter(|o| !o.is_empty()) else {
            return Ok(0);
        };
        let n = self
            .reader
            .read(out)
            .map_err(|_| BlobError::Stream("request body read failed"))?;
        self.left = self
            .left
            .saturating_sub(u64::try_from(n).unwrap_or(u64::MAX));
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        Err(BlobError::Stream("a request body cannot rewind"))
    }
}

/// Writes a status line and headers, ending the head.
pub(crate) fn write_head(
    out: &mut dyn Write,
    status: u16,
    headers: &[(&str, &str)],
) -> std::io::Result<()> {
    write!(out, "HTTP/1.1 {status} {}\r\n", reason(status))?;
    for (name, value) in headers {
        write!(out, "{name}: {value}\r\n")?;
    }
    out.write_all(b"\r\n")
}

/// Writes a whole response whose body is `body`.
pub(crate) fn write_full(
    out: &mut dyn Write,
    status: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    // ALLOC: one number.
    let len = body.len().to_string();
    // ALLOC: the caller's headers plus one.
    let mut all: Vec<(&str, &str)> = Vec::with_capacity(headers.len().saturating_add(1));
    all.extend_from_slice(headers);
    all.push(("content-length", &len));
    write_head(out, status, &all)?;
    out.write_all(body)
}

/// Writes an S3 error document for `code`.
pub(crate) fn write_error(
    out: &mut dyn Write,
    status: u16,
    code: &str,
    close: bool,
    head_only: bool,
) -> std::io::Result<()> {
    // ALLOC: one error document of a fixed shape.
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{code}</Message></Error>"
    );
    // ALLOC: at most three headers.
    let mut headers = vec![("content-type", "application/xml")];
    if close {
        headers.push(("connection", "close"));
    }
    if head_only {
        // ALLOC: one number.
        let len = body.len().to_string();
        headers.push(("content-length", &len));
        return write_head(out, status, &headers);
    }
    write_full(out, status, &headers, body.as_bytes())
}

/// A body sent with `Transfer-Encoding: chunked`, for a LIST page whose
/// length is not known when its first key is written.
pub(crate) struct Chunked<'a> {
    pub(crate) out: &'a mut dyn Write,
}

impl Chunked<'_> {
    /// Sends `bytes` as one chunk; an empty slice sends nothing.
    pub(crate) fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        write!(self.out, "{:x}\r\n", bytes.len())?;
        self.out.write_all(bytes)?;
        self.out.write_all(b"\r\n")
    }

    /// Ends the body.
    pub(crate) fn finish(&mut self) -> std::io::Result<()> {
        self.out.write_all(b"0\r\n\r\n")
    }
}

/// Appends `text` to `out` with XML's five special characters escaped.
pub(crate) fn xml_escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        412 => "Precondition Failed",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding_refuses_bad_escapes() {
        assert_eq!(percent_decode("a%20b%2F%C3%A9").as_deref(), Some("a b/é"));
        for bad in ["%", "%2", "%zz", "%FF"] {
            assert_eq!(percent_decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn xml_specials_are_escaped() {
        let mut out = String::new();
        xml_escape("a&<>\"'é", &mut out);
        assert_eq!(out, "a&amp;&lt;&gt;&quot;&apos;é");
    }

    #[test]
    fn every_status_has_a_reason() {
        for s in [
            200, 204, 206, 400, 403, 404, 405, 409, 411, 412, 416, 500, 503,
        ] {
            assert_ne!(reason(s), "Status", "{s}");
        }
        assert_eq!(reason(299), "Status");
    }

    #[test]
    fn chunks_are_framed_and_empty_ones_skipped() {
        let mut out = Vec::new();
        let mut c = Chunked { out: &mut out };
        c.send(b"").unwrap();
        c.send(b"hello world, sixteen").unwrap();
        c.finish().unwrap();
        assert_eq!(out, b"14\r\nhello world, sixteen\r\n0\r\n\r\n");
    }

    #[test]
    fn an_error_head_carries_the_length_of_the_body_it_omits() {
        let mut out = Vec::new();
        write_error(&mut out, 404, "NoSuchKey", true, true).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("connection: close\r\n"));
        assert!(text.ends_with("\r\n\r\n") && !text.contains("<Error>"));
    }
}
