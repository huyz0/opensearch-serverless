//! The client's conditional requests, on which its registers are built:
//! `If-None-Match: *` and `If-Match` on PUT, `If-Match` on DELETE, and the
//! ETag of what a GET read.

use super::S3Store;
use super::request::{Condition, PutAnswer};
use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::register::known::KnownVersions;
use crate::register::tagged::{Precondition, Tagged};
use crate::store::{ByteRange, check_buf, check_key};

/// Bytes a register body is sent through.
const PUT_BUF: usize = 16_384;

impl S3Store {
    /// A PUT that writes only if `pre` holds, carrying a writer token: the
    /// new object's ETag (empty when the store sent none), or `None` when
    /// the store refused it. A refusal is counted as a failed attempt,
    /// though not an error to the caller. When a retry is refused after an
    /// attempt whose outcome is unknown, that attempt may have stored this
    /// very body: one HEAD reads the object's writer token to tell, and its
    /// ETag.
    pub(super) fn put_if(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        pre: Precondition<'_>,
    ) -> BlobResult<Option<String>> {
        let token = self.writer_token();
        let cond = Condition {
            pre,
            writer: &token,
        };
        let mut last = 0;
        match self.write(key, body, buf, Some(cond), &mut last)? {
            // ALLOC: an empty string allocates nothing.
            PutAnswer::Stored(tag) => return Ok(Some(tag.unwrap_or_default())),
            PutAnswer::Refused => self.meter.record_error(),
        }
        if last == 0 {
            return Ok(None);
        }
        let head = self.run(|_| Some((self.head_once(key, &token), true)))?;
        Ok(match head {
            Some((_, true, tag)) => Some(tag.unwrap_or_default()),
            _ => None,
        })
    }
}

impl Tagged for S3Store {
    fn get_tagged(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<Option<String>> {
        check_key(key)?;
        check_buf(buf)?;
        match self.run(|_| Some(self.get_once(key, range, buf, sink))) {
            Ok((_, Some(tag))) => Ok(Some(tag)),
            Ok((_, None)) => Err(BlobError::Protocol("no ETag")),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn put_tagged(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        pre: Precondition<'_>,
    ) -> BlobResult<Option<String>> {
        let mut buf = [0u8; PUT_BUF];
        self.put_if(key, body, &mut buf, pre)
    }

    fn delete_tagged(&self, key: &str, etag: &str) -> BlobResult<bool> {
        check_key(key)?;
        let deleted = self.run(|_| Some((self.delete_once(key, Some(etag)), true)))?;
        if !deleted {
            // S3 answered 412 or 404: a failed attempt, though not an error.
            self.meter.record_error();
        }
        Ok(deleted)
    }

    fn known(&self) -> &KnownVersions {
        &self.known
    }
}
