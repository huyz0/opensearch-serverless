//! The HTTP/1.1 response from the node's own listener: head, then body in bounded pieces.

use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use tokio::io::{AsyncRead, AsyncReadExt};

/// The response head, `1xx` answers already skipped.
pub struct ResponseHead {
    /// The status.
    pub status: StatusCode,
    /// The headers that survive into HTTP/2.
    pub headers: HeaderMap,
    framing: Framing,
}

impl ResponseHead {
    /// Whether the response ends with its head.
    #[must_use]
    pub fn bodyless(&self) -> bool {
        matches!(self.framing, Framing::None | Framing::Length(0))
    }
}

enum Framing {
    None,
    Length(u64),
    Chunked(Chunked),
    UntilClose,
}

/// Why the upstream's answer cannot be forwarded.
#[derive(Debug)]
pub enum UpstreamError {
    /// The connection failed or ended early.
    Io,
    /// The answer is not valid HTTP/1.1, or its head is over the limit.
    Invalid,
}

/// Reads the response off `rd` through one fixed buffer.
pub struct Response<R> {
    rd: R,
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

impl<R: AsyncRead + Unpin> Response<R> {
    /// A reader with a buffer of `size` bytes, which is also the limit on a response head.
    pub fn new(rd: R, size: usize) -> Self {
        // ALLOC: one buffer per stream, `stream_buffer` bytes, charged at start.
        Self {
            rd,
            buf: vec![0; size],
            start: 0,
            end: 0,
        }
    }

    async fn fill(&mut self) -> Result<usize, UpstreamError> {
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
        if self.end == self.buf.len() {
            return Err(UpstreamError::Invalid);
        }
        let n = self
            .rd
            .read(&mut self.buf[self.end..])
            .await
            .map_err(|_| UpstreamError::Io)?;
        self.end = self.end.saturating_add(n);
        Ok(n)
    }

    /// The final response head: a `1xx` head is read and dropped.
    ///
    /// # Errors
    ///
    /// [`UpstreamError`] when the connection ends first or the head is not valid or does not fit.
    pub async fn head(&mut self, head_request: bool) -> Result<ResponseHead, UpstreamError> {
        loop {
            let mut raw = [httparse::EMPTY_HEADER; 64];
            let mut parsed = httparse::Response::new(&mut raw);
            let status = parsed
                .parse(&self.buf[self.start..self.end])
                .map_err(|_| UpstreamError::Invalid)?;
            if let httparse::Status::Complete(used) = status {
                let code = parsed.code.ok_or(UpstreamError::Invalid)?;
                let status = StatusCode::from_u16(code).map_err(|_| UpstreamError::Invalid)?;
                let mut headers = HeaderMap::new();
                let mut length = None;
                let mut chunked = false;
                for h in parsed.headers.iter() {
                    let name = HeaderName::from_bytes(h.name.as_bytes())
                        .map_err(|_| UpstreamError::Invalid)?;
                    let value =
                        HeaderValue::from_bytes(h.value).map_err(|_| UpstreamError::Invalid)?;
                    match name.as_str() {
                        "connection" | "keep-alive" | "proxy-connection" | "upgrade" => continue,
                        "transfer-encoding" => {
                            chunked = value.as_bytes().eq_ignore_ascii_case(b"chunked");
                            continue;
                        }
                        "content-length" => {
                            length = value
                                .to_str()
                                .ok()
                                .and_then(|s| s.trim().parse::<u64>().ok());
                        }
                        _ => {}
                    }
                    headers.append(name, value);
                }
                self.start = self.start.saturating_add(used);
                if status.is_informational() {
                    continue;
                }
                let framing = if head_request
                    || status == StatusCode::NO_CONTENT
                    || status == StatusCode::NOT_MODIFIED
                {
                    Framing::None
                } else if chunked {
                    headers.remove(http::header::CONTENT_LENGTH);
                    Framing::Chunked(Chunked::default())
                } else if let Some(n) = length {
                    Framing::Length(n)
                } else {
                    Framing::UntilClose
                };
                return Ok(ResponseHead {
                    status,
                    headers,
                    framing,
                });
            }
            if self.fill().await? == 0 {
                return Err(UpstreamError::Io);
            }
        }
    }

    /// The next piece of the body, at most the buffer size; `None` at its end.
    ///
    /// # Errors
    ///
    /// [`UpstreamError`] when the body is malformed or the connection ends before it is complete.
    pub async fn next(&mut self, head: &mut ResponseHead) -> Result<Option<Bytes>, UpstreamError> {
        loop {
            match &mut head.framing {
                Framing::None => return Ok(None),
                Framing::Length(0) => return Ok(None),
                Framing::Length(left) => {
                    if self.start == self.end && self.fill().await? == 0 {
                        return Err(UpstreamError::Io);
                    }
                    let avail = self.end.saturating_sub(self.start);
                    let take = usize::try_from(*left).map_or(avail, |l| l.min(avail));
                    let out = Bytes::copy_from_slice(
                        &self.buf[self.start..self.start.saturating_add(take)],
                    );
                    self.start = self.start.saturating_add(take);
                    *left = left.saturating_sub(take as u64);
                    return Ok(Some(out));
                }
                Framing::UntilClose => {
                    if self.start == self.end && self.fill().await? == 0 {
                        return Ok(None);
                    }
                    let out = Bytes::copy_from_slice(&self.buf[self.start..self.end]);
                    self.start = self.end;
                    return Ok(Some(out));
                }
                Framing::Chunked(state) => {
                    if state.is_done() {
                        return Ok(None);
                    }
                    if self.start == self.end && self.fill().await? == 0 {
                        return Err(UpstreamError::Io);
                    }
                    // ALLOC: at most one buffer of decoded data per call.
                    let mut out = BytesMut::with_capacity(self.end.saturating_sub(self.start));
                    let used = state
                        .decode(&self.buf[self.start..self.end], &mut out)
                        .map_err(|()| UpstreamError::Invalid)?;
                    self.start = self.start.saturating_add(used);
                    if !out.is_empty() {
                        return Ok(Some(out.freeze()));
                    }
                }
            }
        }
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Size,
    Ext,
    SizeLf,
    Data,
    DataCr,
    DataLf,
    Trailer,
    Done,
}

/// A chunked-body decoder: bounded state, data copied out as it is found.
#[derive(Default)]
pub struct Chunked {
    phase: Phase,
    size: u64,
    digits: bool,
    left: u64,
    line_empty: bool,
}

impl Chunked {
    fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// Consumes bytes of `input`, appending body bytes to `out`; returns how many it consumed.
    ///
    /// # Errors
    ///
    /// `Err` when the framing is invalid.
    pub fn decode(&mut self, input: &[u8], out: &mut BytesMut) -> Result<usize, ()> {
        let mut i = 0;
        while i < input.len() && !self.is_done() {
            let b = input[i];
            match self.phase {
                Phase::Size => match b {
                    b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => {
                        let d = u64::from((b as char).to_digit(16).ok_or(())?);
                        self.size = self
                            .size
                            .checked_mul(16)
                            .and_then(|s| s.checked_add(d))
                            .ok_or(())?;
                        self.digits = true;
                        i = i.saturating_add(1);
                    }
                    b';' | b' ' | b'\t' if self.digits => self.phase = Phase::Ext,
                    b'\r' if self.digits => {
                        self.phase = Phase::SizeLf;
                        i = i.saturating_add(1);
                    }
                    _ => return Err(()),
                },
                Phase::Ext => {
                    if b == b'\r' {
                        self.phase = Phase::SizeLf;
                    }
                    i = i.saturating_add(1);
                }
                Phase::SizeLf => {
                    if b != b'\n' {
                        return Err(());
                    }
                    i = i.saturating_add(1);
                    if self.size == 0 {
                        self.phase = Phase::Trailer;
                        self.line_empty = true;
                    } else {
                        self.left = self.size;
                        self.phase = Phase::Data;
                    }
                    self.size = 0;
                    self.digits = false;
                }
                Phase::Data => {
                    let avail = input.len().saturating_sub(i);
                    let take = usize::try_from(self.left).map_or(avail, |l| l.min(avail));
                    out.extend_from_slice(&input[i..i.saturating_add(take)]);
                    i = i.saturating_add(take);
                    self.left = self.left.saturating_sub(take as u64);
                    if self.left == 0 {
                        self.phase = Phase::DataCr;
                    }
                }
                Phase::DataCr => {
                    if b != b'\r' {
                        return Err(());
                    }
                    self.phase = Phase::DataLf;
                    i = i.saturating_add(1);
                }
                Phase::DataLf => {
                    if b != b'\n' {
                        return Err(());
                    }
                    self.phase = Phase::Size;
                    i = i.saturating_add(1);
                }
                Phase::Trailer => {
                    i = i.saturating_add(1);
                    match b {
                        b'\r' => {}
                        b'\n' if self.line_empty => self.phase = Phase::Done,
                        b'\n' => self.line_empty = true,
                        _ => self.line_empty = false,
                    }
                }
                Phase::Done => {}
            }
        }
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(pieces: &[&[u8]]) -> Result<(Vec<u8>, bool), ()> {
        let mut c = Chunked::default();
        let mut out = BytesMut::new();
        for p in pieces {
            let used = c.decode(p, &mut out)?;
            assert_eq!(used, p.len(), "everything is consumed unless done");
        }
        Ok((out.to_vec(), c.is_done()))
    }

    #[test]
    fn chunks_decode_across_any_split() {
        let wire = b"5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nx-trailer: 1\r\n\r\n";
        for cut in 0..wire.len() {
            let (a, b) = wire.split_at(cut);
            let (body, done) = decode_all(&[a, b]).expect("decodes");
            assert_eq!(body, b"hello world", "cut at {cut}");
            assert!(done, "cut at {cut}");
        }
    }

    #[test]
    fn bad_framing_is_refused() {
        for bad in [
            &b"zz\r\n"[..],
            b"5\r\nhelloXX",
            b"\r\n",
            b"ffffffffffffffffff\r\n",
        ] {
            assert!(decode_all(&[bad]).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn heads_skip_continue_and_frame_bodies() {
        let wire: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 3\r\nx: y\r\n\r\nabcdef";
        let mut r = Response::new(wire, 1024);
        let mut h = r.head(false).await.expect("head");
        assert_eq!(h.status, 200);
        assert!(h.headers.get("connection").is_none());
        assert_eq!(h.headers.get("x").expect("x"), "y");
        let b = r.next(&mut h).await.expect("body").expect("some");
        assert_eq!(&b[..], b"abc");
        assert!(r.next(&mut h).await.expect("end").is_none());
    }

    #[tokio::test]
    async fn head_requests_and_close_delimited_bodies() {
        let wire: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\n";
        let mut r = Response::new(wire, 1024);
        let mut h = r.head(true).await.expect("head");
        assert!(r.next(&mut h).await.expect("none").is_none());
        let wire: &[u8] = b"HTTP/1.1 200 OK\r\n\r\nrest";
        let mut r = Response::new(wire, 1024);
        let mut h = r.head(false).await.expect("head");
        assert_eq!(
            &r.next(&mut h).await.expect("b").expect("some")[..],
            b"rest"
        );
        assert!(r.next(&mut h).await.expect("end").is_none());
    }

    #[tokio::test]
    async fn an_oversized_head_is_invalid() {
        let wire = format!("HTTP/1.1 200 OK\r\nx: {}\r\n\r\n", "a".repeat(2000));
        let mut r = Response::new(wire.as_bytes(), 512);
        assert!(matches!(r.head(false).await, Err(UpstreamError::Invalid)));
    }
}
