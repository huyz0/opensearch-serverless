//! The commit of a shard its writer holds but has not published yet.

use oss_blob::{BlobStore, PooledStore};
use oss_format::Register;

use super::{
    IndexMeta, MAX_REGISTER_BYTES, ShardFiles, descriptor_key, index_of, meta_of, read_register,
};
use crate::commit_push::Stage;
use crate::error::ReaderError;

impl ShardFiles {
    /// The empty commit of a shard whose commit register is missing while
    /// its head names a live owner for this incarnation of the index: the
    /// owner took it and its first publish has not landed, so what it
    /// wrote is in its tail and not in any commit yet. Reads the
    /// descriptor and the head, two GETs.
    ///
    /// # Errors
    ///
    /// `NotFound` when the head names no live owner of this incarnation:
    /// a register lost from a shard that was activated is an error, as
    /// [`Self::open`] gives it.
    pub fn open_unpublished_with_meta<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        shard: u32,
        now_millis: i64,
    ) -> Result<(Self, IndexMeta), ReaderError> {
        let descriptor_bytes =
            read_register(store, &descriptor_key(base, index)?, MAX_REGISTER_BYTES)?;
        let descriptor = index_of(&descriptor_bytes)?;
        let meta = meta_of(&descriptor)?;
        if descriptor.bucket.is_some() {
            return Err(ReaderError::Blob(oss_blob::BlobError::NotFound));
        }
        let key = oss_cluster::heads::shard_head_key(base, &descriptor.name, shard);
        let bytes = read_register(store, &key, MAX_REGISTER_BYTES)?;
        let head = oss_format::ShardHead::parse(Register::parse(&bytes)?.value)?;
        let ours = head.index_uuid.as_deref() == Some(&*descriptor.uuid);
        match head.owner_node_id {
            Some(owner) if ours && head.is_held_at(now_millis) => {
                let mut files = Self::never_written();
                // The owner is named as its writer, so a search asks it for
                // the tail the empty commit lacks.
                files.stage = Stage::new(0, Some(owner), -1);
                Ok((files, meta))
            }
            _ => Err(ReaderError::Blob(oss_blob::BlobError::NotFound)),
        }
    }
}
