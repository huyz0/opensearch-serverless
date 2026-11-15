//! What the S3 front end does with one request: verify its signature, then
//! answer it from the emulator as S3 would.

use std::cell::Cell;
use std::io::Write;

use super::Shared;
use super::wire::{Chunked, Head, SocketBody, percent_decode, write_error, write_head, xml_escape};
use super::writes;
use crate::chunk::ChunkSink;
use crate::emulator::{Span, etag};
use crate::error::BlobError;
use crate::error::BlobResult;
use crate::s3::sigv4::{self, Scope};
use crate::store::{BlobStore, ByteRange, MAX_LIST_KEYS};

/// Bytes the front end streams a body through, each way.
pub(super) const BODY_BUF: usize = 16_384;

/// What is left to send once the handler has run and the request body has
/// been read to its end.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    /// The response has been written.
    Done,
    /// A response with no body.
    Empty(u16),
    /// A 200 with no body for a stored object, carrying its ETag.
    Stored(String),
    /// An S3 error document.
    Error(u16, &'static str),
    /// The response was cut part-way; the connection must close.
    Close,
}

/// The status and S3 error code for a store failure.
pub(super) fn status_of(err: &BlobError) -> Reply {
    let (status, code) = match err {
        BlobError::NotFound => (404, "NoSuchKey"),
        BlobError::InvalidRange => (416, "InvalidRange"),
        BlobError::Throttled | BlobError::Busy => (503, "SlowDown"),
        BlobError::Conflict => (409, "ConditionalRequestConflict"),
        BlobError::Unavailable => (500, "InternalError"),
        BlobError::CapacityExceeded => (507, "InsufficientStorage"),
        BlobError::Denied => (403, "AccessDenied"),
        BlobError::Unexpected(_) | BlobError::InvalidArgument(_) | BlobError::Protocol(_) => {
            (400, "InvalidArgument")
        }
        BlobError::Stream(_) => return Reply::Close,
    };
    Reply::Error(status, code)
}

/// A request whose signature checked out: its path and query, decoded.
struct Verified {
    path: String,
    query: Vec<(String, String)>,
}

/// Checks the request's SigV4 `Authorization` header by deriving the
/// canonical request again from what arrived, as S3 does: the path and
/// query decoded and encoded afresh, the signed headers by name.
fn verify(shared: &Shared, head: &Head) -> Result<Verified, &'static str> {
    let auth = head.header("authorization").ok_or("AccessDenied")?;
    let date = head.header("x-amz-date").ok_or("AccessDenied")?;
    let payload = head
        .header("x-amz-content-sha256")
        .ok_or("InvalidRequest")?;
    let signed = auth
        .split_once("SignedHeaders=")
        .and_then(|(_, rest)| rest.split(',').next())
        .ok_or("AuthorizationHeaderMalformed")?;
    if !signed.split(';').any(|n| n == "host") {
        return Err("AuthorizationHeaderMalformed");
    }
    let path = percent_decode(&head.path).ok_or("InvalidURI")?;
    // ALLOC: three bytes per path byte at most.
    let mut uri = String::with_capacity(path.len().saturating_mul(3));
    sigv4::uri_encode(path.as_bytes(), true, &mut uri);
    let pairs = query_pairs(&head.query).ok_or("InvalidURI")?;
    // ALLOC: one entry per query parameter of the request.
    let borrowed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        // ALLOC: as above.
        .collect();
    // ALLOC: one entry per signed header, at most MAX_HEADERS.
    let headers: Vec<(&str, &str)> = signed
        .split(';')
        .flat_map(|name| {
            head.headers
                .iter()
                .filter(move |(n, _)| n == name)
                .map(|(n, v)| (n.as_str(), v.as_str()))
        })
        // ALLOC: as above.
        .collect();
    let canonical = sigv4::canonical_request(
        &head.method,
        &uri,
        &sigv4::canonical_query(&borrowed),
        &headers,
        payload,
    );
    let scope = Scope {
        date: date.get(..8).ok_or("AuthorizationHeaderMalformed")?,
        region: &shared.region,
        service: "s3",
    };
    if sigv4::authorization(&shared.credentials, &scope, date, &canonical) == auth {
        Ok(Verified { path, query: pairs })
    } else {
        Err("SignatureDoesNotMatch")
    }
}

/// The query's decoded name and value pairs.
fn query_pairs(query: &str) -> Option<Vec<(String, String)>> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (n, v) = p.split_once('=').unwrap_or((p, ""));
            Some((percent_decode(n)?, percent_decode(v)?))
        })
        // ALLOC: one pair per query parameter of the request line, inside MAX_LINE.
        .collect()
}

/// `bytes=a-b` or `bytes=a-`, as a range of the emulator.
fn parse_range(value: &str) -> Option<ByteRange> {
    let (first, last) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = first.parse().ok()?;
    if last.is_empty() {
        return Some(ByteRange::From {
            start,
            len: u64::MAX,
        });
    }
    let end: u64 = last.parse().ok()?;
    let len = end.checked_sub(start)?.checked_add(1)?;
    Some(ByteRange::From { start, len })
}

/// Answers one request. `out` is the connection's writer; a body the
/// handler does not read is drained by the caller before `Reply` is sent.
pub(crate) fn respond(
    shared: &Shared,
    head: &Head,
    body: &mut SocketBody<'_>,
    out: &mut dyn Write,
) -> Reply {
    let request = match verify(shared, head) {
        Ok(request) => request,
        Err(code) => return Reply::Error(403, code),
    };
    if let Some((status, code)) = shared.scripted_answer() {
        return Reply::Error(status, code);
    }
    let Some(rest) = request
        .path
        .strip_prefix('/')
        .and_then(|p| p.strip_prefix(shared.bucket.as_str()))
    else {
        return Reply::Error(404, "NoSuchBucket");
    };
    let key = match rest.strip_prefix('/') {
        Some(k) if !k.is_empty() => Some(k),
        Some(_) => None,
        None if rest.is_empty() => None,
        None => return Reply::Error(404, "NoSuchBucket"),
    };
    match (head.method.as_str(), key) {
        ("GET", Some(key)) => get(shared, head, key, out),
        ("HEAD", Some(key)) => writes::head(shared, key, out),
        ("PUT", Some(key)) => writes::put(shared, head, key, body),
        ("DELETE", Some(key)) => writes::delete(shared, head, key),
        ("GET", None) => list(shared, &request.query, out),
        _ => Reply::Error(405, "MethodNotAllowed"),
    }
}

/// A GET's response: its head, written before the first chunk (or at the
/// end for an empty body), then the chunks, cut short when scripted.
struct GetReply<'a> {
    out: &'a mut dyn Write,
    span: &'a Cell<Option<Span>>,
    ranged: bool,
    head_written: bool,
    cut: Option<u64>,
}

impl GetReply<'_> {
    /// Writes the head, once; later calls do nothing.
    fn head(&mut self) -> BlobResult<()> {
        if self.head_written {
            return Ok(());
        }
        let Some(span) = self.span.get() else {
            return Err(BlobError::Stream("no span before the body"));
        };
        self.head_written = true;
        // ALLOC: one number.
        let len = span.len.to_string();
        let last = span
            .first
            .saturating_add(span.len)
            .saturating_sub(1)
            .max(span.first);
        // ALLOC: one header value of three numbers.
        let range = format!("bytes {}-{last}/{}", span.first, span.object_len);
        let tag = etag(span.tag);
        let mut headers = [
            ("content-length", len.as_str()),
            ("etag", tag.as_str()),
            ("content-range", &range),
        ];
        let (status, headers) = if self.ranged {
            (206, &mut headers[..])
        } else {
            (200, headers.get_mut(..2).unwrap_or_default())
        };
        write_head(self.out, status, headers).map_err(|_| BlobError::Stream("client went away"))
    }
}

impl ChunkSink for GetReply<'_> {
    fn accept(&mut self, chunk: &[u8]) -> BlobResult<()> {
        self.head()?;
        let chunk = match self.cut {
            Some(left) => {
                let n = usize::try_from(left).unwrap_or(usize::MAX).min(chunk.len());
                self.cut = Some(left.saturating_sub(u64::try_from(n).unwrap_or(u64::MAX)));
                chunk.get(..n).unwrap_or_default()
            }
            None => chunk,
        };
        self.out
            .write_all(chunk)
            .map_err(|_| BlobError::Stream("client went away"))?;
        if self.cut == Some(0) {
            let _ = self.out.flush();
            return Err(BlobError::Stream("body cut"));
        }
        Ok(())
    }
}

fn get(shared: &Shared, head: &Head, key: &str, out: &mut dyn Write) -> Reply {
    let range = match head.header("range") {
        None => ByteRange::Full,
        Some(value) => match parse_range(value) {
            Some(range) => range,
            None => return Reply::Error(400, "InvalidArgument"),
        },
    };
    let span = Cell::new(None);
    let mut reply = GetReply {
        out,
        span: &span,
        ranged: range != ByteRange::Full,
        head_written: false,
        cut: shared.take_cut(),
    };
    let mut start = |s| {
        span.set(Some(s));
        Ok(())
    };
    let mut buf = [0u8; BODY_BUF];
    match shared
        .store
        .get_framed(key, range, &mut buf, &mut start, &mut reply)
    {
        // An empty body has no chunk to write the head before.
        Ok(_) => match reply.head() {
            Ok(()) => Reply::Done,
            Err(_) => Reply::Close,
        },
        // Once the head is out, the only failures are the sink's own,
        // `Stream` errors, which close the connection.
        Err(err) => status_of(&err),
    }
}

/// One `ListObjectsV2` page, written as chunks while the emulator visits
/// its keys.
fn list(shared: &Shared, pairs: &[(String, String)], out: &mut dyn Write) -> Reply {
    let param = |name: &str| {
        pairs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    if param("list-type") != Some("2") {
        return Reply::Error(400, "InvalidArgument");
    }
    let prefix = param("prefix").unwrap_or_default();
    let start_after = param("start-after");
    let Ok(max) = param("max-keys").map_or(Ok(MAX_LIST_KEYS), str::parse::<usize>) else {
        return Reply::Error(400, "InvalidArgument");
    };
    // ALLOC: one entry or document head at a time, a few KiB.
    let mut text = String::with_capacity(4096);
    let mut failed = false;
    let mut started = false;
    let mut chunked = Chunked { out };
    let begin = |chunked: &mut Chunked<'_>, text: &mut String| -> std::io::Result<()> {
        write_head(
            chunked.out,
            200,
            &[
                ("content-type", "application/xml"),
                ("transfer-encoding", "chunked"),
            ],
        )?;
        text.clear();
        text.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListBucketResult><Name>");
        xml_escape(&shared.bucket, text);
        text.push_str("</Name><Prefix>");
        xml_escape(prefix, text);
        text.push_str("</Prefix>");
        chunked.send(text.as_bytes())
    };
    // LIST: the front end serving the one page the client asked for.
    let store = &shared.store;
    let result = store.list_prefix(prefix, start_after, max, &mut |key, size| {
        if failed {
            return;
        }
        let mut step = || -> std::io::Result<()> {
            if !started {
                started = true;
                begin(&mut chunked, &mut text)?;
            }
            text.clear();
            text.push_str("<Contents><Key>");
            xml_escape(key, &mut text);
            text.push_str("</Key><Size>");
            // ALLOC: one number.
            text.push_str(&size.to_string());
            text.push_str("</Size><StorageClass>STANDARD</StorageClass></Contents>");
            chunked.send(text.as_bytes())
        };
        failed = step().is_err();
    });
    let page = match result {
        Ok(page) => page,
        Err(err) => return status_of(&err),
    };
    let mut end = || -> std::io::Result<()> {
        if !started {
            begin(&mut chunked, &mut text)?;
        }
        text.clear();
        text.push_str("<KeyCount>");
        // ALLOC: one number.
        text.push_str(&page.keys.to_string());
        text.push_str("</KeyCount><IsTruncated>");
        text.push_str(if page.truncated { "true" } else { "false" });
        text.push_str("</IsTruncated></ListBucketResult>");
        chunked.send(text.as_bytes())?;
        chunked.finish()
    };
    if failed || end().is_err() {
        return Reply::Close;
    }
    Reply::Done
}

/// Writes what [`respond`] left to send, if anything.
pub(crate) fn finish(
    reply: &Reply,
    out: &mut dyn Write,
    close: bool,
    head_only: bool,
) -> std::io::Result<()> {
    match reply {
        Reply::Done | Reply::Close => Ok(()),
        Reply::Empty(status) => {
            // ALLOC: two headers.
            let mut headers = vec![("content-length", "0")];
            if close {
                headers.push(("connection", "close"));
            }
            write_head(out, *status, &headers)
        }
        Reply::Stored(tag) => {
            // ALLOC: three headers.
            let mut headers = vec![("content-length", "0"), ("etag", tag.as_str())];
            if close {
                headers.push(("connection", "close"));
            }
            write_head(out, 200, &headers)
        }
        Reply::Error(status, code) => write_error(out, *status, code, close, head_only),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_parse_as_s3_writes_them() {
        assert_eq!(
            parse_range("bytes=0-0"),
            Some(ByteRange::From { start: 0, len: 1 })
        );
        assert_eq!(
            parse_range("bytes=5-"),
            Some(ByteRange::From {
                start: 5,
                len: u64::MAX
            })
        );
        for bad in [
            "bytes=5-4",
            "bytes=a-1",
            "items=0-1",
            "bytes=0",
            "bytes=0-x",
        ] {
            assert_eq!(parse_range(bad), None, "{bad}");
        }
    }

    #[test]
    fn every_failure_has_a_status() {
        assert_eq!(status_of(&BlobError::Stream("x")), Reply::Close);
        assert_eq!(status_of(&BlobError::Busy), Reply::Error(503, "SlowDown"));
        assert_eq!(
            status_of(&BlobError::Denied),
            Reply::Error(403, "AccessDenied")
        );
        assert_eq!(
            status_of(&BlobError::Protocol("x")),
            Reply::Error(400, "InvalidArgument")
        );
    }
}
