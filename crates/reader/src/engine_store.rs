//! The engine's view of a published commit: lucene-rust's `BlobStore` over
//! `oss-blob`, where each Lucene file is a range of a shell blob.
//!
//! lucene-rust's `CachedBlobDirectory` reads every file through
//! [`lucene_store::BlobStore::get_range_into`]; here that becomes one ranged
//! GET of the blob holding the file, streamed into the page cache's buffer.
//! Listing and lengths come from the commit's [`ShardFiles`], so opening a
//! commit sends no LIST and no HEAD. The blob key is the file's version: a
//! shell blob is immutable, so cached pages can never go stale. The store
//! is read-only: a reader never writes.

use std::io::{self, ErrorKind};
use std::sync::Arc;

use lucene_store::{Error, Input, Result};
use oss_blob::{BlobError, BlobStore, ByteRange, PooledStore};

use crate::shard_files::{FileRange, ShardFiles};

/// A read-only lucene-rust store over one commit's files.
#[derive(Debug)]
pub struct EngineStore<S> {
    store: Arc<PooledStore<S>>,
    files: ShardFiles,
}

impl<S: BlobStore> EngineStore<S> {
    /// The engine's store for `files`, read through `store`.
    pub fn new(store: Arc<PooledStore<S>>, files: ShardFiles) -> Self {
        Self { store, files }
    }

    /// The commit this store serves.
    pub fn files(&self) -> &ShardFiles {
        &self.files
    }

    fn range(&self, name: &str) -> Result<&FileRange> {
        self.files
            .get(name)
            .ok_or_else(|| lucene_store::blob_directory::not_found(name))
    }
}

/// An engine error for a store failure, keeping "not found" recognisable.
fn engine_error(e: BlobError) -> Error {
    let kind = match e {
        BlobError::NotFound => ErrorKind::NotFound,
        BlobError::Busy | BlobError::Throttled => ErrorKind::WouldBlock,
        _ => ErrorKind::Other,
    };
    Error::Io(io::Error::new(kind, e))
}

fn read_only(name: &str) -> Error {
    // ALLOC: one message on a refused write, which a reader never makes.
    Error::IllegalArgument(format!("the reader's store is read-only: {name}"))
}

impl<S: BlobStore> lucene_store::BlobStore for EngineStore<S> {
    fn list(&self) -> Result<Vec<String>> {
        // ALLOC: one name a commit file, at most the manifest's file limit.
        Ok(self.files.names().map(str::to_owned).collect())
    }

    fn get(&self, name: &str) -> Result<Input> {
        // ALLOC: the message below, only on a file too large for memory.
        let len = usize::try_from(self.range(name)?.length)
            .map_err(|_| Error::IllegalArgument(format!("{name} is too large to read whole")))?;
        // ALLOC: one whole file; the page cache asks for this only for small
        // and pinned files (metadata, term index), never segment data.
        let mut bytes = vec![0u8; len];
        let n = self.get_range_into(name, 0, &mut bytes)?;
        bytes.truncate(n);
        Ok(Input::Owned(bytes))
    }

    fn get_range_into(&self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize> {
        let file = self.range(name)?;
        if offset >= file.length || out.is_empty() {
            return Ok(0);
        }
        let want = u64::try_from(out.len())
            .unwrap_or(u64::MAX)
            .min(file.length.saturating_sub(offset));
        let start = file
            .offset
            .checked_add(offset)
            // ALLOC: one message, only on a corrupt offset.
            .ok_or_else(|| Error::Corrupted(format!("{name}: offset overflows")))?;
        let mut filled = 0usize;
        self.store
            .get(
                &file.key,
                ByteRange::From { start, len: want },
                &mut |chunk: &[u8]| {
                    let end = filled.saturating_add(chunk.len());
                    let dest = out
                        .get_mut(filled..end)
                        .ok_or(BlobError::Stream("store sent more than was asked"))?;
                    dest.copy_from_slice(chunk);
                    filled = end;
                    Ok(())
                },
            )
            .map_err(engine_error)?;
        Ok(filled)
    }

    fn list_with_sizes(&self) -> Result<Option<Vec<(String, u64)>>> {
        Ok(Some(
            self.files
                .names()
                // ALLOC: one entry a commit file, at most the manifest's file limit.
                .map(|n| (n.to_owned(), self.files.get(n).map_or(0, |f| f.length)))
                .collect(),
        ))
    }

    fn size(&self, name: &str) -> Result<u64> {
        Ok(self.range(name)?.length)
    }

    fn version(&self, name: &str) -> Result<Option<String>> {
        // ALLOC: one key, asked once per file open.
        Ok(Some(self.range(name)?.key.clone()))
    }

    fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.files.get(name).is_some())
    }

    fn put(&self, name: &str, _bytes: Vec<u8>) -> Result<()> {
        Err(read_only(name))
    }

    fn delete(&self, name: &str) -> Result<()> {
        Err(read_only(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(e: BlobError) -> ErrorKind {
        match engine_error(e) {
            Error::Io(io) => io.kind(),
            other => panic!("an io error: {other}"),
        }
    }

    #[test]
    fn store_failures_keep_a_kind_the_engine_can_act_on() {
        assert_eq!(kind(BlobError::NotFound), ErrorKind::NotFound);
        assert!(engine_error(BlobError::NotFound).is_no_such_file());
        assert_eq!(kind(BlobError::Busy), ErrorKind::WouldBlock);
        assert_eq!(kind(BlobError::Throttled), ErrorKind::WouldBlock);
        assert_eq!(kind(BlobError::Unavailable), ErrorKind::Other);
    }
}
