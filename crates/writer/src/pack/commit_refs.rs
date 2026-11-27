//! The reference source over commits: whether the latest commit of a shard
//! with a block in a pack records a file in it.
//!
//! Ported from `NodePackCollector`'s commit scan
//! (huyz0/OpenSearch `feature/serverlessshell`), which reads the pack's
//! header for the shards that wrote into it and then each shard's commit.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ByteRange};
use oss_format::node_pack_format::{PREFIX_BYTES, header_end};
use oss_format::{
    CommitManifest, DescriptorRecord, FileLocation, NodePackHeader, Register, ShardRef,
    bucket_manifest_key, descriptor_key, node_pack_key, parse_bucket_member,
};

use super::{PackError, PackId, PackReferences};

/// The most a register is read as, as the reader's bound.
const MAX_REGISTER_BYTES: usize = 4 << 20;

/// Whether a frozen view holds an index incarnation whole: a live point in
/// time or a snapshot of it. There is no implementation that answers "no"
/// without looking: a record that cannot be read is an error, and the pass
/// stops. The answer must come from records read after the pass began, so
/// that a view made after a pack was first seen unreferenced is in it.
pub trait IndexHolds {
    /// Whether a point in time or a snapshot holds incarnation `uuid` of
    /// `index`.
    ///
    /// # Errors
    ///
    /// A record that could not be read, or more records than the caps allow.
    fn holds(&self, index: &str, uuid: &str) -> Result<bool, PackError>;
}

/// Whether any shard's latest commit, or any frozen view of one, names a
/// pack. A view holds its incarnation's packs whole, as the janitor holds
/// its blobs: coarser than the files it names, and never finer.
pub struct CommitReferences {
    store: Arc<dyn BlobStore>,
    base: String,
    holds: Arc<dyn IndexHolds>,
}

impl CommitReferences {
    /// A source over the commits under `base`.
    ///
    /// # Errors
    ///
    /// [`PackError::Format`] for a base that is not empty and does not end
    /// in `/`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        holds: Arc<dyn IndexHolds>,
    ) -> Result<Self, PackError> {
        descriptor_key(base, "probe")?;
        Ok(Self {
            store,
            // ALLOC: the base path, once a source.
            base: base.to_owned(),
            holds,
        })
    }

    /// Up to `len` bytes of `key` from the start; `None` when it is absent.
    fn leading(&self, key: &str, len: usize) -> Result<Option<Vec<u8>>, PackError> {
        // ALLOC: one read, at most a pack header (4 MiB) or its prefix.
        let mut out = Vec::new();
        let range = ByteRange::From {
            start: 0,
            len: len as u64,
        };
        match self
            .store
            .get(key, range, &mut [0u8; 16 << 10], &mut |c: &[u8]| {
                if out.len().saturating_add(c.len()) > len {
                    return Err(BlobError::Stream("more bytes than the range asked for"));
                }
                out.extend_from_slice(c);
                Ok(())
            }) {
            Ok(_) => Ok(Some(out)),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// A register's value, `None` when absent.
    fn register(&self, key: &str) -> Result<Option<Vec<u8>>, PackError> {
        // ALLOC: one register, at most MAX_REGISTER_BYTES, checked as it fills.
        let mut out = Vec::new();
        match self.store.get(
            key,
            ByteRange::Full,
            &mut [0u8; 16 << 10],
            &mut |c: &[u8]| {
                if out.len().saturating_add(c.len()) > MAX_REGISTER_BYTES {
                    return Err(BlobError::Stream("register too large"));
                }
                out.extend_from_slice(c);
                Ok(())
            },
        ) {
            Ok(_) => Ok(Some(out)),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The latest commit of one shard, `None` when there is none.
    fn commit(&self, name: &str, uuid: &str, shard: u32) -> Result<Option<Vec<u8>>, PackError> {
        let Some(blob) = self.register(&descriptor_key(&self.base, name)?)? else {
            return Ok(None);
        };
        let descriptor = match DescriptorRecord::parse(Register::parse(&blob)?.value)? {
            DescriptorRecord::Index(d) if d.uuid == uuid => d,
            // A deleted, aliased or recreated name holds nothing of this shard.
            _ => return Ok(None),
        };
        if let Some(bucket) = descriptor.bucket.as_deref() {
            let Some(manifest) = self.register(&bucket_manifest_key(&self.base, bucket)?)? else {
                return Ok(None);
            };
            let entry = parse_bucket_member(Register::parse(&manifest)?.value, uuid)?;
            return Ok(entry.member.and_then(|m| m.commit));
        }
        let key = ShardRef {
            index: name,
            uuid,
            shard,
        }
        .manifest_key(&self.base)?;
        let Some(blob) = self.register(&key)? else {
            return Ok(None);
        };
        // ALLOC: one commit, at most MAX_REGISTER_BYTES, once a shard asked about.
        Ok(Some(Register::parse(&blob)?.value.to_vec()))
    }
}

impl PackReferences for CommitReferences {
    fn names(&self, pack: &PackId<'_>) -> Result<bool, PackError> {
        let key = node_pack_key(&self.base, pack.node, pack.epoch, pack.ordinal)?;
        let Some(prefix) = self.leading(&key, PREFIX_BYTES)? else {
            return Ok(false);
        };
        let end = header_end(&prefix)?;
        let Some(leading) = self.leading(&key, end)? else {
            return Ok(false);
        };
        let header = NodePackHeader::parse(&leading)?;
        // ALLOC: one entry a distinct shard in one header, at most 4 MiB / 30 bytes.
        let mut seen: Vec<(&str, &str, u32)> = Vec::new();
        for block in header.blocks() {
            let shard = (&*block.index_name, &*block.index_uuid, block.shard);
            if seen.contains(&shard) {
                continue;
            }
            seen.push(shard);
            if self.holds.holds(shard.0, shard.1)? {
                return Ok(true);
            }
            let Some(commit) = self.commit(shard.0, shard.1, shard.2)? else {
                continue;
            };
            let manifest = CommitManifest::parse(&commit)?;
            let named = manifest.files().iter().any(|f| {
                matches!(
                    f.location,
                    FileLocation::NodePacked { node, epoch, ordinal, .. }
                        if node == pack.node && epoch == pack.epoch && ordinal == pack.ordinal
                )
            });
            if named {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
