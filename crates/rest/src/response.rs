//! Responses, streamed through one fixed buffer.
//!
//! A handler starts a response with its status, content type and length,
//! then writes the body through [`BodyWriter`], which implements
//! `std::io::Write`. Bytes collect in the worker's write buffer and go to
//! the socket each time it fills: as raw bytes under a `content-length`, or
//! as one HTTP chunk each when the length is not known in advance. Nothing
//! holds the whole body.
//!
//! The headers follow `DefaultRestChannel#sendResponse`
//! (server/src/main/java/org/opensearch/http/DefaultRestChannel.java at
//! huyz0/OpenSearch@49f238ed): `X-Opaque-Id` and `X-Request-Id` are echoed,
//! and a `HEAD` response carries the length a `GET` would, without the body.

use std::io::{self, Write};

use crate::error::RestError;
use crate::method::MethodSet;

/// How the body's length is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// Exactly this many bytes, sent with `content-length`.
    Known(u64),
    /// Unknown in advance: sent chunked.
    Chunked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    NotStarted,
    Known(u64),
    Chunked,
    /// A body of unknown length to an HTTP/1.0 client: sent raw, ended by
    /// closing the connection.
    Unframed,
    Finished,
}

/// The JSON content type OpenSearch answers with.
pub const JSON: &str = "application/json; charset=UTF-8";

/// The response to one request.
pub struct Response<'a> {
    out: &'a mut dyn Write,
    buf: &'a mut [u8],
    len: usize,
    head_len: usize,
    state: State,
    head_only: bool,
    http10: bool,
    connection: Option<&'static str>,
    echo: [(&'static str, Option<&'a str>); 2],
}

impl std::fmt::Debug for Response<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<'a> Response<'a> {
    pub(crate) fn new(
        out: &'a mut dyn Write,
        buf: &'a mut [u8],
        head_only: bool,
        connection: Option<&'static str>,
        opaque_id: Option<&'a str>,
        request_id: Option<&'a str>,
    ) -> Self {
        Self {
            out,
            buf,
            len: 0,
            head_len: 0,
            state: State::NotStarted,
            head_only,
            http10: false,
            connection,
            echo: [("x-opaque-id", opaque_id), ("x-request-id", request_id)],
        }
    }

    /// Marks the client as HTTP/1.0, which cannot read a chunked body: one
    /// of unknown length is sent raw and the connection closes after it.
    pub(crate) fn set_http10(&mut self) {
        self.http10 = true;
    }

    /// Whether the connection must close after this response.
    pub(crate) fn closes(&self) -> bool {
        self.connection == Some("close")
    }

    /// Sends the status line and headers and returns the body writer.
    /// `headers` are added as given. An echoed `X-Opaque-Id` or
    /// `X-Request-Id` longer than a quarter of the write buffer is left out,
    /// so the head always fits.
    pub fn start(
        &mut self,
        status: u16,
        content_type: Option<&str>,
        length: Length,
        headers: &[(&str, &str)],
    ) -> Result<BodyWriter<'_, 'a>, RestError> {
        if self.state != State::NotStarted {
            return Err(RestError::from(io::Error::other(
                "the response already started",
            )));
        }
        let unframed = length == Length::Chunked && self.http10;
        if unframed {
            self.connection = Some("close");
        }
        let max_echo = self.buf.len().checked_div(4).unwrap_or_default();
        let mut head = Cursor::new(&mut self.buf[..]);
        write!(head, "HTTP/1.1 {status} {}\r\n", reason_phrase(status))?;
        let echoed = self.echo.iter().filter_map(|(n, v)| v.map(|v| (n, v)));
        for (name, value) in echoed.filter(|(_, v)| v.len() <= max_echo) {
            write!(head, "{name}: {value}\r\n")?;
        }
        for (name, value) in headers {
            write!(head, "{name}: {value}\r\n")?;
        }
        if let Some(ct) = content_type {
            write!(head, "content-type: {ct}\r\n")?;
        }
        match length {
            Length::Known(n) => write!(head, "content-length: {n}\r\n")?,
            Length::Chunked if unframed => {}
            Length::Chunked => head.write_all(b"transfer-encoding: chunked\r\n")?,
        }
        if let Some(c) = self.connection {
            write!(head, "connection: {c}\r\n")?;
        }
        head.write_all(b"\r\n")?;
        self.len = head.pos;
        self.head_len = head.pos;
        self.state = match length {
            Length::Known(n) => State::Known(n),
            Length::Chunked if unframed => State::Unframed,
            Length::Chunked => State::Chunked,
        };
        if self.head_only || length == Length::Known(0) {
            self.flush_raw()?;
        }
        Ok(BodyWriter { resp: self })
    }

    /// Sends a whole small body with its length.
    pub fn send(&mut self, status: u16, content_type: &str, body: &[u8]) -> Result<(), RestError> {
        let len = u64::try_from(body.len()).unwrap_or(u64::MAX);
        let mut w = self.start(status, Some(content_type), Length::Known(len), &[])?;
        w.write_all(body)?;
        w.finish()
    }

    /// Sends `e` with its status, headers and body.
    pub fn send_error(&mut self, e: &RestError) -> Result<(), RestError> {
        if e.closes_connection() {
            self.connection = Some("close");
        }
        let mut counter = Counter(0);
        e.write_body(&mut counter)?;
        // ALLOC: none; the Allow value is written into a fixed array.
        let mut allow = FixedStr::<64>::default();
        e.allow()
            .write_joined(&mut allow, ",")
            .map_err(|_| io::Error::other("allow"))?;
        let allow_header = [("allow", allow.as_str())];
        let headers: &[(&str, &str)] = if e.allow() == MethodSet::EMPTY {
            &[]
        } else {
            &allow_header
        };
        let content_type = if e.is_empty() { None } else { Some(JSON) };
        let mut w = self.start(e.status(), content_type, Length::Known(counter.0), headers)?;
        e.write_body(&mut w)?;
        w.finish()
    }

    /// Whether the status line has been sent.
    pub fn is_started(&self) -> bool {
        self.state != State::NotStarted
    }

    /// Whether the response is complete.
    pub fn is_finished(&self) -> bool {
        self.state == State::Finished
    }

    fn flush_raw(&mut self) -> io::Result<()> {
        self.out.write_all(&self.buf[..self.len])?;
        self.len = 0;
        self.head_len = 0;
        Ok(())
    }

    /// Sends the buffered bytes: the head if still buffered, then the body
    /// bytes raw, or as one chunk.
    fn flush_body(&mut self) -> io::Result<()> {
        if self.head_only {
            // The head went out when the response started.
            self.len = 0;
            return Ok(());
        }
        let payload = self.len.saturating_sub(self.head_len);
        if self.state != State::Chunked || payload == 0 {
            return self.flush_raw();
        }
        self.out.write_all(&self.buf[..self.head_len])?;
        write!(self.out, "{payload:x}\r\n")?;
        self.out.write_all(&self.buf[self.head_len..self.len])?;
        self.len = 0;
        self.head_len = 0;
        self.out.write_all(b"\r\n")
    }

    fn write_body(&mut self, data: &[u8]) -> io::Result<usize> {
        if let State::Known(left) = self.state {
            let n = u64::try_from(data.len()).unwrap_or(u64::MAX);
            if n > left {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "body longer than its content-length",
                ));
            }
            self.state = State::Known(left.saturating_sub(n));
        }
        let mut rest = data;
        while !rest.is_empty() {
            if self.len == self.buf.len() {
                self.flush_body()?;
            }
            let room = self.buf.len().saturating_sub(self.len);
            let n = room.min(rest.len());
            let end = self.len.saturating_add(n);
            self.buf[self.len..end].copy_from_slice(&rest[..n]);
            self.len = end;
            rest = &rest[n..];
        }
        Ok(data.len())
    }

    fn finish(&mut self) -> Result<(), RestError> {
        match self.state {
            State::Known(0) => {}
            State::Known(_) => {
                return Err(RestError::from(io::Error::other(
                    "body shorter than its content-length",
                )));
            }
            State::Chunked => {
                self.flush_body()?;
                if !self.head_only {
                    self.out.write_all(b"0\r\n\r\n")?;
                }
            }
            State::Unframed => {}
            State::NotStarted | State::Finished => {
                return Err(RestError::from(io::Error::other(
                    "no response body to finish",
                )));
            }
        }
        self.flush_body()?;
        self.out.flush()?;
        self.state = State::Finished;
        Ok(())
    }
}

/// Writes a response body; see [`Response::start`].
pub struct BodyWriter<'r, 'a> {
    resp: &'r mut Response<'a>,
}

impl BodyWriter<'_, '_> {
    /// Sends what is buffered and ends the body. A known-length body must
    /// have been written in full.
    pub fn finish(self) -> Result<(), RestError> {
        self.resp.finish()
    }
}

impl Write for BodyWriter<'_, '_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.resp.write_body(data)
    }

    /// Sends the buffered bytes now, as one chunk when chunked.
    fn flush(&mut self) -> io::Result<()> {
        self.resp.flush_body()?;
        self.resp.out.flush()
    }
}

/// The reason phrase for `status`.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        415 => "Unsupported Media Type",
        417 => "Expectation Failed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// Counts bytes written.
struct Counter(u64);

impl Write for Counter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer over a fixed slice; writing past its end is an error.
struct Cursor<'b> {
    buf: &'b mut [u8],
    pos: usize,
}

impl<'b> Cursor<'b> {
    fn new(buf: &'b mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }
}

impl Write for Cursor<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let end = self
            .pos
            .checked_add(data.len())
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| io::Error::other("response head larger than the write buffer"))?;
        self.buf[self.pos..end].copy_from_slice(data);
        self.pos = end;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A string in a fixed array.
#[derive(Debug)]
struct FixedStr<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for FixedStr<N> {
    fn default() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> FixedStr<N> {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or_default()
    }
}

impl<const N: usize> std::fmt::Write for FixedStr<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self
            .len
            .checked_add(s.len())
            .filter(|&e| e <= N)
            .ok_or(std::fmt::Error)?;
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
#[path = "response_tests.rs"]
mod tests;
