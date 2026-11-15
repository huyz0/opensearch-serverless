//! The emulator's conditional requests, on which its registers are built.

use super::{Emulator, etag};
use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::register::known::KnownVersions;
use crate::register::tagged::{Precondition, Tagged};
use crate::store::ByteRange;

impl Tagged for Emulator {
    fn get_tagged(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<Option<String>> {
        let mut tag = 0u64;
        match self.get_framed(
            key,
            range,
            buf,
            &mut |span| {
                tag = span.tag;
                Ok(())
            },
            sink,
        ) {
            Ok(_) => Ok(Some(etag(tag))),
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
        let mut buf = [0u8; 4096];
        let written = self.put_if(key, body, &mut buf, Some(pre), None)?;
        Ok(written.map(etag))
    }

    fn delete_tagged(&self, key: &str, wanted: &str) -> BlobResult<bool> {
        self.delete_if(key, wanted)
    }

    fn known(&self) -> &KnownVersions {
        &self.known
    }
}
