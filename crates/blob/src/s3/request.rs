//! One attempt of each S3 request: sign it, send it, read the answer.
//!
//! Each `*_once` records exactly one attempt in the meter, just before the
//! request goes out, and answers whether a failure may be repeated: a GET
//! or LIST that has already handed bytes or keys to the caller may not, so
//! the caller never sees anything twice. The status mapping of a refused
//! conditional write is ported from `S3BlobContainer.isConditionFailure`
//! in the Java shell (`plugins/repository-s3/src/main/java/org/opensearch/repositories/s3/S3BlobContainer.java@49f238ed`).

use std::io::Read;
use std::time::SystemTime;

use ureq::SendBody;
use ureq::http::{self, Request, Response};

use super::S3Store;
use super::sigv4::{self, Scope, UNSIGNED_PAYLOAD};
use super::xml::{ListParser, error_code};
use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::op::S3Op;
use crate::store::{ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta};

/// The user-metadata header a conditional create stores its writer token
/// in; S3 returns it on HEAD.
const WRITER: &str = "x-amz-meta-writer";
/// Bytes of an error body read for its `<Code>`; S3's are a few hundred.
const ERROR_BODY: usize = 1024;

/// Bytes read at a time from a LIST response.
const LIST_CHUNK: usize = 4096;

/// What a non-success status means for the request that got it.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// A conditional write's precondition was evaluated and not met.
    Taken,
    /// Anything else.
    Failed(BlobError),
}

/// Maps a status and an error code to what it means, as the Java shell
/// does: 412, or 409 with `PreconditionFailed`, proves the condition was
/// not met; every other 409 (`ConditionalRequestConflict`) leaves it
/// unknown and is retried.
fn classify(status: u16, code: Option<&str>) -> Refusal {
    Refusal::Failed(match status {
        412 => return Refusal::Taken,
        409 if code == Some("PreconditionFailed") => return Refusal::Taken,
        409 => BlobError::Conflict,
        404 => BlobError::NotFound,
        416 => BlobError::InvalidRange,
        401 | 403 => BlobError::Denied,
        429 | 503 => BlobError::Throttled,
        500 | 502 | 504 => BlobError::Unavailable,
        other => BlobError::Unexpected(other),
    })
}

/// The refusal a response carries, reading at most [`ERROR_BODY`] bytes of
/// its body for the error code.
fn refusal(response: &mut Response<ureq::Body>) -> Refusal {
    let status = response.status().as_u16();
    let mut body = [0u8; ERROR_BODY];
    let mut filled = 0usize;
    let mut reader = response.body_mut().as_reader();
    while let Some(rest) = body.get_mut(filled..) {
        match reader.read(rest) {
            Ok(n) if n > 0 => filled = filled.saturating_add(n),
            _ => break,
        }
    }
    classify(status, error_code(body.get(..filled).unwrap_or_default()))
}

/// The refusal of a request that has no precondition: a 412 there is a
/// status it did not expect.
fn failure(response: &mut Response<ureq::Body>) -> BlobError {
    match refusal(response) {
        Refusal::Taken => BlobError::Unexpected(response.status().as_u16()),
        Refusal::Failed(err) => err,
    }
}

fn header<'a>(response: &'a Response<ureq::Body>, name: &str) -> Option<&'a str> {
    response.headers().get(name)?.to_str().ok()
}

fn content_length(response: &Response<ureq::Body>) -> BlobResult<u64> {
    header(response, "content-length")
        .and_then(|v| v.trim().parse().ok())
        .ok_or(BlobError::Protocol("no Content-Length"))
}

/// The whole object's length from `Content-Range: bytes a-b/total`.
fn range_total(response: &Response<ureq::Body>) -> BlobResult<u64> {
    header(response, "content-range")
        .and_then(|v| v.rsplit_once('/'))
        .and_then(|(_, total)| total.trim().parse().ok())
        .ok_or(BlobError::Protocol("no Content-Range total"))
}

/// A PUT body as `ureq` reads it: filled from the source into the caller's
/// buffer, and checked against its declared length before its last byte
/// goes out, so a body that is too long or too short never completes and
/// the store commits nothing.
struct Body<'a> {
    source: &'a mut dyn ChunkSource,
    buf: &'a mut [u8],
    filled: usize,
    pos: usize,
    sent: u64,
    len: u64,
    /// The source's failure, kept so the caller gets it rather than the
    /// I/O error `ureq` wraps it in.
    failure: Option<BlobError>,
}

impl Body<'_> {
    fn refill(&mut self) -> BlobResult<()> {
        let n = self.source.fill(self.buf)?;
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        let sent = self.sent.saturating_add(n64);
        if n == 0 {
            return Err(BlobError::InvalidArgument(
                "body shorter than its declared length",
            ));
        }
        if n > self.buf.len() {
            return Err(BlobError::Stream("source filled past the buffer"));
        }
        if sent > self.len || (sent == self.len && self.source.fill(&mut [0u8; 1])? != 0) {
            return Err(BlobError::InvalidArgument(
                "body longer than its declared length",
            ));
        }
        self.sent = sent;
        self.filled = n;
        self.pos = 0;
        Ok(())
    }
}

impl Read for Body<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.filled {
            if self.sent >= self.len {
                return Ok(0);
            }
            if let Err(err) = self.refill() {
                self.failure = Some(err);
                return Err(std::io::Error::other("PUT body failed"));
            }
        }
        let ready = self.buf.get(self.pos..self.filled).unwrap_or_default();
        let n = ready.len().min(out.len());
        if let (Some(dst), Some(src)) = (out.get_mut(..n), ready.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.pos = self.pos.saturating_add(n);
        Ok(n)
    }
}

impl S3Store {
    /// Signs and sends one request, recording it as one attempt of `op`.
    /// `key` is the object, or `None` for the bucket; `query` pairs are
    /// decoded. A connection that fails, or a body that fails to stream,
    /// is an error here; any status is a response.
    fn send(
        &self,
        op: S3Op,
        method: http::Method,
        key: Option<&str>,
        query: &[(&str, &str)],
        extra: &[(&str, &str)],
        body: Option<&mut Body<'_>>,
    ) -> BlobResult<Response<ureq::Body>> {
        self.meter.record(op);
        // ALLOC: the bucket and an encoded key, at most 3 * MAX_KEY_BYTES.
        let mut path = String::with_capacity(
            self.bucket
                .len()
                .saturating_add(key.map_or(0, |k| k.len().saturating_mul(3)))
                .saturating_add(2),
        );
        path.push('/');
        path.push_str(&self.bucket);
        if let Some(key) = key {
            path.push('/');
            sigv4::uri_encode(key.as_bytes(), true, &mut path);
        }
        let query = sigv4::canonical_query(query);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let date = sigv4::amz_date(now);
        // ALLOC: at most a dozen headers per request.
        let mut signed: Vec<(&str, &str)> = Vec::with_capacity(extra.len().saturating_add(3));
        signed.push(("host", &self.host));
        signed.push(("x-amz-content-sha256", UNSIGNED_PAYLOAD));
        signed.push(("x-amz-date", &date));
        signed.extend_from_slice(extra);
        let canonical =
            sigv4::canonical_request(method.as_str(), &path, &query, &signed, UNSIGNED_PAYLOAD);
        let scope = Scope {
            date: date.get(..8).unwrap_or_default(),
            region: &self.region,
            service: "s3",
        };
        let authorization = sigv4::authorization(&self.credentials, &scope, &date, &canonical);
        let url = if query.is_empty() {
            // ALLOC: the origin and the path.
            format!("{}{path}", self.origin)
        } else {
            // ALLOC: the origin, the path and the query.
            format!("{}{path}?{query}", self.origin)
        };
        let mut builder = Request::builder().method(method).uri(url);
        // Every signed header is sent as signed, Host included.
        for (name, value) in &signed {
            builder = builder.header(*name, *value);
        }
        builder = builder.header("authorization", authorization);
        match body {
            None => builder
                .body(())
                .map_err(|_| BlobError::InvalidArgument("request did not build"))
                .and_then(|r| self.agent.run(r).map_err(|_| BlobError::Unavailable)),
            Some(body) => {
                let result = builder
                    .body(SendBody::from_reader(&mut *body))
                    .map_err(|_| BlobError::InvalidArgument("request did not build"))
                    .and_then(|r| self.agent.run(r).map_err(|_| BlobError::Unavailable));
                match body.failure.take() {
                    Some(err) => Err(err),
                    None => result,
                }
            }
        }
    }

    pub(super) fn get_once(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> (BlobResult<GetOutcome>, bool) {
        let header = match range {
            ByteRange::Full => None,
            ByteRange::From { start, len } => {
                Some(match start.checked_add(len.saturating_sub(1)) {
                    // ALLOC: one header value of two numbers.
                    Some(end) => format!("bytes={start}-{end}"),
                    // ALLOC: one header value of one number.
                    None => format!("bytes={start}-"),
                })
            }
        };
        // ALLOC: at most one header.
        let extra: Vec<(&str, &str)> = header.iter().map(|h| ("range", h.as_str())).collect();
        let mut response =
            match self.send(range.op(), http::Method::GET, Some(key), &[], &extra, None) {
                Ok(r) => r,
                Err(err) => return (Err(err), true),
            };
        let status = response.status().as_u16();
        let object_len = match (status, range) {
            (200, ByteRange::Full) => content_length(&response),
            (206, ByteRange::From { .. }) => range_total(&response),
            (200 | 206, _) => Err(BlobError::Protocol(
                "response does not match the range asked",
            )),
            _ => return (Err(failure(&mut response)), true),
        };
        let (object_len, expected) = match (object_len, content_length(&response)) {
            (Ok(total), Ok(body)) => (total, body),
            (Err(err), _) | (_, Err(err)) => return (Err(err), true),
        };
        let mut reader = response.body_mut().as_reader();
        let mut bytes = 0u64;
        loop {
            let n = match reader.read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => return (Err(BlobError::Unavailable), bytes == 0),
            };
            let Some(chunk) = buf.get(..n) else {
                return (Err(BlobError::Stream("read past the buffer")), false);
            };
            if let Err(err) = sink.accept(chunk) {
                return (Err(err), false);
            }
            let n = u64::try_from(n).unwrap_or(u64::MAX);
            self.meter.add_bytes_read(n);
            bytes = bytes.saturating_add(n);
        }
        // ureq fails a read that ends before Content-Length, so this is a
        // guard against a transport that does not; it is never retried.
        if bytes != expected {
            return (Err(BlobError::Unavailable), false);
        }
        (Ok(GetOutcome { object_len, bytes }), true)
    }

    pub(super) fn put_once(
        &self,
        key: &str,
        source: &mut dyn ChunkSource,
        buf: &mut [u8],
        create: Option<&str>,
    ) -> (BlobResult<CreateOutcome>, bool) {
        let if_absent = create.is_some();
        let len = source.len();
        // ALLOC: one number.
        let length = len.to_string();
        let mut extra = [
            ("content-length", length.as_str()),
            ("if-none-match", "*"),
            (WRITER, create.unwrap_or_default()),
        ];
        let extra = extra
            .get_mut(..if if_absent { 3 } else { 1 })
            .unwrap_or_default();
        let mut body = Body {
            source,
            buf,
            filled: 0,
            pos: 0,
            sent: 0,
            len,
            failure: None,
        };
        let sent = self.send(
            S3Op::Put,
            http::Method::PUT,
            Some(key),
            &[],
            extra,
            Some(&mut body),
        );
        let mut response = match sent {
            Ok(r) => r,
            // The caller's source failed: repeating it is the caller's call.
            Err(err @ (BlobError::Stream(_) | BlobError::InvalidArgument(_))) => {
                return (Err(err), false);
            }
            Err(err) => return (Err(err), true),
        };
        match response.status().as_u16() {
            200 | 201 | 204 => {
                self.meter.add_bytes_written(len);
                (Ok(CreateOutcome::Created), true)
            }
            _ => match refusal(&mut response) {
                Refusal::Taken if if_absent => (Ok(CreateOutcome::AlreadyExists), true),
                Refusal::Taken => (Err(BlobError::Unexpected(response.status().as_u16())), true),
                Refusal::Failed(err) => (Err(err), true),
            },
        }
    }

    /// One HEAD; with the object, whether its writer token is `writer`.
    pub(super) fn head_once(
        &self,
        key: &str,
        writer: &str,
    ) -> BlobResult<Option<(ObjectMeta, bool)>> {
        let mut response = self.send(S3Op::Head, http::Method::HEAD, Some(key), &[], &[], None)?;
        match response.status().as_u16() {
            200 => Ok(Some((
                ObjectMeta {
                    len: content_length(&response)?,
                },
                header(&response, WRITER) == Some(writer),
            ))),
            404 => Ok(None),
            _ => Err(failure(&mut response)),
        }
    }

    pub(super) fn delete_once(&self, key: &str) -> BlobResult<()> {
        let mut response = self.send(
            S3Op::Delete,
            http::Method::DELETE,
            Some(key),
            &[],
            &[],
            None,
        )?;
        match response.status().as_u16() {
            200 | 204 => Ok(()),
            _ => Err(failure(&mut response)),
        }
    }

    pub(super) fn list_once(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> (BlobResult<ListPage>, bool) {
        // ALLOC: one number.
        let max_keys = max.to_string();
        let mut query = [
            ("list-type", "2"),
            ("max-keys", max_keys.as_str()),
            ("prefix", prefix),
            ("start-after", start_after.unwrap_or_default()),
        ];
        let query = query
            .get_mut(..if start_after.is_some() { 4 } else { 3 })
            .unwrap_or_default();
        // LIST: one bounded page, as the caller asked.
        let mut response = match self.send(S3Op::List, http::Method::GET, None, query, &[], None) {
            Ok(r) => r,
            Err(err) => return (Err(err), true),
        };
        if response.status().as_u16() != 200 {
            return (Err(failure(&mut response)), true);
        }
        let mut parser = ListParser::new(max);
        let mut chunk = [0u8; LIST_CHUNK];
        let mut reader = response.body_mut().as_reader();
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => return (Err(BlobError::Unavailable), parser.keys() == 0),
            };
            if let Err(err) = parser.feed(chunk.get(..n).unwrap_or_default(), visit) {
                return (Err(err), false);
            }
        }
        let result = parser.finish().map(|truncated| ListPage {
            keys: parser.keys(),
            truncated,
        });
        // A page that ended cleanly fails only as a `Protocol` error, which
        // is never retried.
        (result, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_as_the_java_shell_maps_them() {
        assert_eq!(classify(412, None), Refusal::Taken);
        assert_eq!(classify(409, Some("PreconditionFailed")), Refusal::Taken);
        let failed = |e| Refusal::Failed(e);
        assert_eq!(
            classify(409, Some("ConditionalRequestConflict")),
            failed(BlobError::Conflict)
        );
        assert_eq!(classify(409, None), failed(BlobError::Conflict));
        assert_eq!(classify(404, None), failed(BlobError::NotFound));
        assert_eq!(classify(416, None), failed(BlobError::InvalidRange));
        for s in [401, 403] {
            assert_eq!(classify(s, None), failed(BlobError::Denied));
        }
        for s in [429, 503] {
            assert_eq!(classify(s, Some("SlowDown")), failed(BlobError::Throttled));
        }
        for s in [500, 502, 504] {
            assert_eq!(classify(s, None), failed(BlobError::Unavailable));
        }
        for s in [400, 405, 411, 501, 507] {
            assert_eq!(classify(s, None), failed(BlobError::Unexpected(s)));
        }
    }
}
