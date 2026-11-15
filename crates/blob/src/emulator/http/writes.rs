//! The S3 front end's HEAD, PUT and DELETE: conditional on `If-Match` or
//! `If-None-Match: *`, and answering with the object's ETag.

use std::io::Write;

use super::Shared;
use super::routes::{BODY_BUF, Reply, status_of};
use super::wire::{Head, SocketBody, write_head};
use crate::emulator::etag;
use crate::register::tagged::Precondition;
use crate::store::BlobStore;

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
        Ok(None) => Reply::Error(412, "PreconditionFailed"),
        Err(err) => status_of(&err),
    }
}

/// Deletes the object, if it carries the `If-Match` ETag when one is sent.
pub(super) fn delete(shared: &Shared, head: &Head, key: &str) -> Reply {
    match head.header("if-match") {
        Some(tag) => match shared.store.delete_if(key, tag) {
            Ok(true) => Reply::Empty(204),
            Ok(false) => Reply::Error(412, "PreconditionFailed"),
            Err(err) => status_of(&err),
        },
        None => match shared.store.delete(key) {
            Ok(()) => Reply::Empty(204),
            Err(err) => status_of(&err),
        },
    }
}
