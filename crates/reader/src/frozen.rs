//! A shard's commit as a point in time froze it: the files come from the
//! view, so the shard's commit register is never read.
//!
//! Ported from `ShardReaders.openFrozenReader`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/shard/ShardReaders.java`):
//! the incarnation check, then the view's own manifest for the shard.

use std::collections::BTreeMap;

use oss_blob::BlobStore;
use oss_blob::PooledStore;
use oss_format::{FileLocation, PointInTime, ShardRef, descriptor_key};

use crate::commit_push::Stage;
use crate::error::ReaderError;
use crate::shard_files::{
    FileRange, IndexMeta, MAX_REGISTER_BYTES, ShardFiles, index_of, meta_of, read_register,
};

impl ShardFiles {
    /// Reads the descriptor of `index` under `base` (one GET) and resolves
    /// the files `view` froze for `shard`, with no request for them.
    ///
    /// # Errors
    ///
    /// [`ReaderError::NotHeld`] when `view` was taken over another
    /// incarnation of the name; [`ReaderError::NotFrozen`] when it is still
    /// being captured or holds nothing of `shard`; [`ReaderError::UnknownLength`]
    /// for a file recorded without a length; a format error for a location no
    /// reader can follow; and as [`ShardFiles::open`] for the descriptor.
    pub fn open_frozen<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        view: &PointInTime,
        shard: u32,
    ) -> Result<(Self, IndexMeta), ReaderError> {
        let bytes = read_register(store, &descriptor_key(base, index)?, MAX_REGISTER_BYTES)?;
        let descriptor = index_of(&bytes)?;
        let meta = meta_of(&descriptor)?;
        if view
            .index_uuid
            .as_deref()
            .is_some_and(|held| held != meta.uuid)
        {
            return Err(ReaderError::NotHeld);
        }
        let frozen = view
            .shards
            .iter()
            .find(|s| s.shard == shard && !view.capturing)
            .ok_or(ReaderError::NotFrozen(shard))?;
        let at = descriptor.shard(shard)?;
        // ALLOC: one entry a frozen file, at most the manifest's file limit.
        let mut files = BTreeMap::new();
        for file in &frozen.files {
            let location = FileLocation::parse(&file.location)?;
            let (key, offset) = ShardRef { ..at }.blob_key(base, &file.name, &location)?;
            let length = file
                .length
                // ALLOC: the name, only for a file recorded without a length.
                .ok_or_else(|| ReaderError::UnknownLength(file.name.clone()))?;
            files.insert(
                // ALLOC: one name a frozen file.
                file.name.clone(),
                FileRange {
                    key,
                    offset,
                    length,
                },
            );
        }
        let stage = Stage::new(frozen.term, None, -1);
        Ok((Self::assembled(frozen.term, stage, files), meta))
    }
}
