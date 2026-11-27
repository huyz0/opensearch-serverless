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
use super::body::Body;
use super::sigv4::{self, Scope, UNSIGNED_PAYLOAD};
use super::xml::{ListParser, error_code};
use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::http_date;
use crate::op::S3Op;
use crate::register::tagged::Precondition;
use crate::store::{ByteRange, GetOutcome, ListPage, ObjectMeta};

/// The user-metadata header a conditional write stores its writer token
/// in; S3 returns it on HEAD.
const WRITER: &str = "x-amz-meta-writer";
/// Bytes of an error body read for its `<Code>`; S3's are a few hundred.
pub(super) const ERROR_BODY: usize = 1024;

/// Bytes read at a time from a LIST response.
const LIST_CHUNK: usize = 4096;

/// What a PUT attempt ended with.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PutAnswer {
    /// Stored, with the new object's ETag if the response carried one.
    Stored(Option<String>),
    /// A conditional PUT whose precondition did not hold.
    Refused,
}

/// A conditional write: what it requires, and the writer token it stores.
#[derive(Clone, Copy, Debug)]
pub(super) struct Condition<'a> {
    /// What the object at the key must be.
    pub(super) pre: Precondition<'a>,
    /// The token a retry looks for to recognise its own write.
    pub(super) writer: &'a str,
}

/// The response's ETag, if it has one.
fn etag(response: &Response<ureq::Body>) -> Option<String> {
    // ALLOC: one ETag, bounded by the connection's header buffer.
    header(response, "etag").map(str::to_owned)
}

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
pub(super) fn failure(response: &mut Response<ureq::Body>) -> BlobError {
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

impl S3Store {
    /// Signs and sends one request, recording it as one attempt of `op`.
    /// `key` is the object, or `None` for the bucket; `query` pairs are
    /// decoded. A connection that fails, or a body that fails to stream,
    /// is an error here; any status is a response.
    pub(super) fn send(
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
    ) -> (BlobResult<(GetOutcome, Option<String>)>, bool) {
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
        let tag = etag(&response);
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
        (Ok((GetOutcome { object_len, bytes }, tag)), true)
    }

    pub(super) fn put_once(
        &self,
        key: &str,
        source: &mut dyn ChunkSource,
        buf: &mut [u8],
        cond: Option<Condition<'_>>,
    ) -> (BlobResult<PutAnswer>, bool) {
        let len = source.len();
        // ALLOC: one number.
        let length = len.to_string();
        let mut extra = [
            ("content-length", length.as_str()),
            ("if-none-match", "*"),
            (WRITER, ""),
        ];
        let used = match cond {
            None => 1,
            Some(Condition { pre, writer }) => {
                if let Precondition::Matches(tag) = pre {
                    extra[1] = ("if-match", tag);
                }
                extra[2].1 = writer;
                3
            }
        };
        let extra = extra.get_mut(..used).unwrap_or_default();
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
                (Ok(PutAnswer::Stored(etag(&response))), true)
            }
            // `If-Match` on a key with no object: the version it names is
            // gone, which is the precondition failing (`isConditionFailure`).
            404 if cond.is_some_and(|c| matches!(c.pre, Precondition::Matches(_))) => {
                (Ok(PutAnswer::Refused), true)
            }
            _ => match refusal(&mut response) {
                Refusal::Taken if cond.is_some() => (Ok(PutAnswer::Refused), true),
                Refusal::Taken => (Err(BlobError::Unexpected(response.status().as_u16())), true),
                Refusal::Failed(err) => (Err(err), true),
            },
        }
    }

    /// One HEAD; with the object, whether its writer token is `writer`,
    /// and its ETag.
    pub(super) fn head_once(
        &self,
        key: &str,
        writer: &str,
    ) -> BlobResult<Option<(ObjectMeta, bool, Option<String>)>> {
        let mut response = self.send(S3Op::Head, http::Method::HEAD, Some(key), &[], &[], None)?;
        match response.status().as_u16() {
            200 => Ok(Some((
                ObjectMeta {
                    len: content_length(&response)?,
                },
                header(&response, WRITER) == Some(writer),
                etag(&response),
            ))),
            404 => Ok(None),
            _ => Err(failure(&mut response)),
        }
    }

    /// One HEAD for the object's age in milliseconds: the response's `Date`
    /// less its `Last-Modified`, both stamped by the store.
    pub(super) fn age_once(&self, key: &str) -> BlobResult<Option<u64>> {
        let mut response = self.send(S3Op::Head, http::Method::HEAD, Some(key), &[], &[], None)?;
        match response.status().as_u16() {
            200 => {
                let date = header(&response, "date").and_then(http_date::parse);
                let modified = header(&response, "last-modified").and_then(http_date::parse);
                match (date, modified) {
                    (Some(date), Some(modified)) => {
                        Ok(Some(date.saturating_sub(modified).saturating_mul(1_000)))
                    }
                    _ => Err(BlobError::Protocol("no Date or Last-Modified")),
                }
            }
            404 => Ok(None),
            _ => Err(failure(&mut response)),
        }
    }

    /// One DELETE, with `If-Match: if_match` when given: `false` when the
    /// object was not that version, or (conditional only) not there.
    pub(super) fn delete_once(&self, key: &str, if_match: Option<&str>) -> BlobResult<bool> {
        let extra = [("if-match", if_match.unwrap_or_default())];
        let extra = extra
            .get(..usize::from(if_match.is_some()))
            .unwrap_or_default();
        let mut response = self.send(
            S3Op::Delete,
            http::Method::DELETE,
            Some(key),
            &[],
            extra,
            None,
        )?;
        match (response.status().as_u16(), if_match) {
            (200 | 204, _) => Ok(true),
            (404, Some(_)) => Ok(false),
            (_, None) => Err(failure(&mut response)),
            (_, Some(_)) => match refusal(&mut response) {
                Refusal::Taken => Ok(false),
                Refusal::Failed(err) => Err(err),
            },
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
