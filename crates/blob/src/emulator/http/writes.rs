//! The S3 front end's HEAD, PUT and DELETE: conditional on `If-Match` or
//! `If-None-Match: *`, and answering with the object's ETag.

use std::io::Write;

use super::Shared;
use super::routes::{BODY_BUF, Reply, status_of};
use super::wire::{Head, SocketBody, write_full, write_head, xml_escape};
use crate::chunk::ChunkSource;
use crate::emulator::etag;
use crate::register::tagged::Precondition;
use crate::s3::delete::{base64, xml_char};
use crate::s3::xml::decode;
use crate::store::BlobStore;
use crate::store::MAX_DELETE_KEYS;

/// The user-metadata header S3 stores with an object and returns on HEAD,
/// which the client uses for its writer token.
pub(super) const WRITER: &str = "x-amz-meta-writer";

/// The object's length, ETag and writer token, if it has one.
pub(super) fn head(shared: &Shared, key: &str, out: &mut dyn Write) -> Reply {
    match shared.store.head_with_writer(key) {
        Ok(Some((meta, tag, writer))) => {
            // ALLOC: one number.
            let len = meta.len.to_string();
            let tag = etag(tag);
            let mut headers = [
                ("content-length", len.as_str()),
                ("etag", tag.as_str()),
                (WRITER, ""),
            ];
            let headers = match &writer {
                Some(w) => {
                    headers[2].1 = w;
                    &headers[..]
                }
                None => &headers[..2],
            };
            match write_head(out, 200, headers) {
                Ok(()) => Reply::Done,
                Err(_) => Reply::Close,
            }
        }
        Ok(None) => Reply::Error(404, "NoSuchKey"),
        Err(err) => status_of(&err),
    }
}

/// The precondition a write's headers carry, or `Err` for a pair S3
/// refuses: anything but `*` in `If-None-Match`.
fn precondition(head: &Head) -> Result<Option<Precondition<'_>>, Reply> {
    match (head.header("if-none-match"), head.header("if-match")) {
        (Some("*"), None) => Ok(Some(Precondition::Absent)),
        (None, Some(tag)) => Ok(Some(Precondition::Matches(tag))),
        (None, None) => Ok(None),
        _ => Err(Reply::Error(501, "NotImplemented")),
    }
}

/// A refused conditional write as S3 answers it: 404 to `If-Match` when
/// there is no object, 412 otherwise.
fn refused(shared: &Shared, key: &str, pre: Option<Precondition<'_>>) -> Reply {
    match pre {
        Some(Precondition::Matches(_)) if !shared.store.contains(key) => {
            Reply::Error(404, "NoSuchKey")
        }
        _ => Reply::Error(412, "PreconditionFailed"),
    }
}

/// Stores the body, if the precondition holds.
pub(super) fn put(shared: &Shared, head: &Head, key: &str, body: &mut SocketBody<'_>) -> Reply {
    if head.header("content-length").is_none() {
        return Reply::Error(411, "MissingContentLength");
    }
    let pre = match precondition(head) {
        Ok(pre) => pre,
        Err(reply) => return reply,
    };
    let mut buf = [0u8; BODY_BUF];
    let writer = head.header(WRITER);
    match shared.store.put_if(key, body, &mut buf, pre, writer) {
        Ok(Some(tag)) => Reply::Stored(etag(tag)),
        Ok(None) => refused(shared, key, pre),
        Err(err) => status_of(&err),
    }
}

/// Deletes the object, if it carries the `If-Match` ETag when one is sent.
pub(super) fn delete(shared: &Shared, head: &Head, key: &str) -> Reply {
    match head.header("if-match") {
        Some(tag) => match shared.store.delete_if(key, tag) {
            Ok(true) => Reply::Empty(204),
            Ok(false) => refused(shared, key, Some(Precondition::Matches(tag))),
            Err(err) => status_of(&err),
        },
        None => match shared.store.delete(key) {
            Ok(()) => Reply::Empty(204),
            Err(err) => status_of(&err),
        },
    }
}

/// The largest batch-delete body taken: a thousand keys of 1,024 bytes,
/// each escaped as six-byte entities, with room for the markup.
const MAX_BATCH_BODY: u64 = 8 << 20;

/// A batch delete (`POST /bucket?delete`): checks the body's
/// `Content-MD5` as S3 does, deletes every key it names in one attempt, and
/// answers an empty quiet result.
pub(super) fn delete_batch(
    shared: &Shared,
    head: &Head,
    body: &mut SocketBody<'_>,
    out: &mut dyn Write,
) -> Reply {
    let Some(md5) = head.header("content-md5") else {
        return Reply::Error(400, "InvalidRequest");
    };
    if body.len() > MAX_BATCH_BODY {
        return Reply::Error(400, "MalformedXML");
    }
    // ALLOC: one batch-delete body, at most MAX_BATCH_BODY.
    let mut xml = vec![0u8; usize::try_from(body.len()).unwrap_or(0)];
    let mut filled = 0usize;
    while let Some(rest) = xml.get_mut(filled..).filter(|r| !r.is_empty()) {
        match body.fill(rest) {
            Ok(n) if n > 0 => filled = filled.saturating_add(n),
            _ => return Reply::Error(400, "IncompleteBody"),
        }
    }
    if base64(&<md5::Md5 as md5::Digest>::digest(&xml)) != md5 {
        return Reply::Error(400, "BadDigest");
    }
    let Some(keys) = batch_keys(&xml) else {
        return Reply::Error(400, "MalformedXML");
    };
    // ALLOC: one answer, an empty result or one key's error.
    let mut result = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DeleteResult>");
    if let Some(code) = shared.scripted_delete_error() {
        result.push_str("<Error><Key>");
        xml_escape(keys.first().map_or("", String::as_str), &mut result);
        result.push_str("</Key><Code>");
        result.push_str(code);
        result.push_str("</Code><Message>scripted</Message></Error>");
    } else {
        // ALLOC: as above, borrowed.
        let named: Vec<&str> = keys.iter().map(String::as_str).collect();
        if let Err(err) = shared.store.delete_batch(&named) {
            return status_of(&err);
        }
    }
    result.push_str("</DeleteResult>");
    match write_full(
        out,
        200,
        &[("content-type", "application/xml")],
        result.as_bytes(),
    ) {
        Ok(()) => Reply::Done,
        Err(_) => Reply::Close,
    }
}

/// The keys a batch-delete body names, decoded; `None` for none, more
/// than [`MAX_DELETE_KEYS`], or a key that does not decode.
fn batch_keys(xml: &[u8]) -> Option<Vec<String>> {
    const OPEN: &[u8] = b"<Key>";
    const CLOSE: &[u8] = b"</Key>";
    // ALLOC: the keys of one batch, at most MAX_DELETE_KEYS.
    let mut keys = Vec::new();
    // ALLOC: one decoded key, at most MAX_KEY_BYTES.
    let mut decoded = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.windows(OPEN.len()).position(|w| w == OPEN) {
        let text = rest.get(at.saturating_add(OPEN.len())..)?;
        let end = text.windows(CLOSE.len()).position(|w| w == CLOSE)?;
        decode(&normalized(text.get(..end)?), &mut decoded).ok()?;
        let key = String::from_utf8(decoded.clone()).ok()?;
        if key.chars().any(|c| !xml_char(c)) {
            return None;
        }
        // ALLOC: as above, one a key.
        keys.push(key);
        rest = text.get(end.saturating_add(CLOSE.len())..)?;
    }
    (!keys.is_empty() && keys.len() <= MAX_DELETE_KEYS).then_some(keys)
}

/// Element text as an XML parser hands it on: raw line breaks (`\r\n` or a
/// lone `\r`) become `\n`. Escaped breaks (`&#13;`) are decoded afterwards
/// and kept; a character XML 1.0 does not allow is refused once decoded.
fn normalized(raw: &[u8]) -> Vec<u8> {
    // ALLOC: one key's text, at most its raw length.
    let mut out = Vec::with_capacity(raw.len());
    let mut bytes = raw.iter().copied().peekable();
    while let Some(b) = bytes.next() {
        match b {
            b'\r' => {
                bytes.next_if_eq(&b'\n');
                out.push(b'\n');
            }
            _ => out.push(b),
        }
    }
    out
}
