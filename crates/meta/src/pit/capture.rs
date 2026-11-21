//! Freezing an index: the capture marker first, then each shard's commit,
//! then the finished view under the same id.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/PointInTimeHandler.java`
//! (`freeze`) and `PointsInTime.beginPointInTime`/`verifyCaptureIncarnation`.
//! The marker is written before any manifest is read, so from the first
//! read the collector deletes nothing for the index and the commit each
//! shard is frozen at cannot be swept out from under the freeze.

use std::fmt;

use oss_blob::BlobError;
use oss_format::{
    CommitManifest, PitFile, PitShard, PointInTime, ShardRef, UnitLayoutRecord,
    bucket_manifest_key, parse_bucket_member,
};

use super::PointInTimeStore;
use crate::descriptors::{DescriptorStore, Target};
use crate::error::MetaError;

/// Why an index could not be frozen.
#[derive(Debug)]
pub enum CaptureError {
    /// Nothing is registered under the name.
    NoSuchIndex,
    /// The name is an alias, which freezes only index by index.
    IsAlias,
    /// The index is one this slice cannot freeze.
    Unsupported(&'static str),
    /// A shard has published nothing, so there is no commit to freeze, and a
    /// view of the rest would silently cover part of the index.
    NothingPublished(u32),
    /// The index was deleted or created again while the capture began.
    Replaced,
    /// A register or a record failed.
    Meta(MetaError),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchIndex => f.write_str("no such index"),
            Self::IsAlias => f.write_str("the name is an alias, not one index"),
            Self::Unsupported(why) => f.write_str(why),
            Self::NothingPublished(shard) => write!(
                f,
                "shard {shard} has published nothing yet, so there is no commit to freeze"
            ),
            Self::Replaced => {
                f.write_str("the index was deleted or recreated while starting capture")
            }
            Self::Meta(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CaptureError {}

impl<E: Into<MetaError>> From<E> for CaptureError {
    fn from(e: E) -> Self {
        Self::Meta(e.into())
    }
}

/// The shards that hold documents now, ascending: `0..slots` for an index
/// without a shard mode, the live unit ids of its layout otherwise (a unit a
/// split retired keeps its slot but is not served, and neither are the
/// halves of a split still pending), as `IndexDescriptor.unitIds`.
fn live_units(layout: Option<&[u8]>, slots: u32) -> Result<Vec<u32>, MetaError> {
    let Some(bytes) = layout else {
        // ALLOC: one id a shard, at most the index record's shard count.
        return Ok((0..slots).collect());
    };
    let layout = UnitLayoutRecord::parse(bytes)?;
    // ALLOC: one id a live unit, at most MAX_SHARDS.
    let mut ids: Vec<u32> = layout
        .units
        .iter()
        // `parse` refuses a negative id, so none is dropped here.
        .filter_map(|unit| u32::try_from(unit.id).ok())
        // ALLOC: the ids counted above.
        .collect();
    ids.sort_unstable();
    Ok(ids)
}

/// A fresh id: 16 random bytes in URL-safe base64 without padding, the
/// Java's `UUIDs.randomBase64UUID`.
///
/// # Errors
///
/// [`BlobError::Unavailable`] when the OS has no randomness.
pub fn new_id() -> Result<String, BlobError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| BlobError::Unavailable)?;
    Ok(encode_id(&bytes))
}

/// 16 bytes as 22 URL-safe base64 symbols, without padding.
fn encode_id(bytes: &[u8; 16]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    // ALLOC: one id, 22 bytes, once a capture.
    let mut id = String::with_capacity(22);
    for chunk in bytes.chunks(3) {
        let mut word = 0u32;
        for (byte, shift) in chunk.iter().zip([16u32, 8, 0]) {
            word |= u32::from(*byte) << shift;
        }
        // A chunk of n bytes gives n + 1 symbols.
        for shift in [18u32, 12, 6, 0]
            .into_iter()
            .take(chunk.len().saturating_add(1))
        {
            let sextet = usize::try_from((word >> shift) & 0x3f).unwrap_or(0);
            id.push(char::from(ALPHABET.get(sextet).copied().unwrap_or(b'A')));
        }
    }
    id
}

impl PointInTimeStore {
    /// Freezes the current commit of every shard of `index` for
    /// `keep_alive_millis`, under a fresh id, and returns the view.
    ///
    /// Order, as the Java's: the capture marker, then a check that the
    /// index is still the incarnation resolved, then one manifest read a
    /// shard, then the finished view replacing the marker. Any failure after
    /// the marker releases it, so a refused capture does not pin the index
    /// for the keep-alive.
    ///
    /// `buf` holds one record or manifest at a time and must hold the
    /// largest; for a bucketed index that is the whole bucket's manifest
    /// register, every member's commit, which a bucket too large for it
    /// fails as a store error with nothing left pinned. Cost: 1 GET for the descriptor and 1 to check it again, a
    /// create's 1 HEAD and 2 PUT, 1 GET a shard, and a finish's 1 GET and 1
    /// PUT: the shards and a constant, once a capture.
    ///
    /// # Errors
    ///
    /// [`CaptureError`]; nothing stays pinned on one.
    pub fn capture(
        &self,
        descriptors: &DescriptorStore,
        index: &str,
        keep_alive_millis: u64,
        buf: &mut [u8],
    ) -> Result<PointInTime, CaptureError> {
        let (uuid, shards, bucket) = match descriptors.resolve(index, buf)?.target {
            Target::Index(record) => {
                // A bucketed index is one shard, whose commit is in its bucket.
                let units = if record.bucket.is_some() {
                    // ALLOC: the one shard, once a capture.
                    vec![0]
                } else {
                    live_units(record.unit_layout, record.number_of_shards)?
                };
                // ALLOC: the uuid and bucket, once a capture.
                (
                    record.uuid.into_owned(),
                    units,
                    record.bucket.map(std::borrow::Cow::into_owned),
                )
            }
            Target::Alias(_) => return Err(CaptureError::IsAlias),
            Target::Absent => return Err(CaptureError::NoSuchIndex),
        };
        let id = new_id().map_err(MetaError::Blob)?;
        let expires = (self.clock)().saturating_add(keep_alive_millis);
        let marker = PointInTime {
            // ALLOC: the capture's names, once a capture.
            id: id.clone(),
            index: index.to_owned(),
            index_uuid: Some(uuid.clone()),
            expires_at_millis: expires,
            capturing: true,
            // ALLOC: empty until finished.
            shards: Vec::new(),
        };
        self.create(&marker, buf)?;
        match self.freeze(descriptors, &marker, shards, bucket.as_deref(), buf) {
            Ok(view) => Ok(view),
            Err(failure) => {
                // Best effort: the failure being reported is the first.
                let _ = self.release(&id, buf);
                Err(failure)
            }
        }
    }

    fn freeze(
        &self,
        descriptors: &DescriptorStore,
        marker: &PointInTime,
        shards: Vec<u32>,
        bucket: Option<&str>,
        buf: &mut [u8],
    ) -> Result<PointInTime, CaptureError> {
        let uuid = marker.index_uuid.as_deref().unwrap_or_default();
        let still = match descriptors.resolve(&marker.index, buf)?.target {
            // The bucket too: a member moved to another bucket is not the commit read from this one.
            Target::Index(record) => record.uuid == uuid && record.bucket.as_deref() == bucket,
            Target::Alias(_) | Target::Absent => false,
        };
        if !still {
            return Err(CaptureError::Replaced);
        }
        // ALLOC: one entry a shard, at most the index record's shard count.
        let mut frozen: Vec<PitShard> = Vec::new();
        for shard in shards {
            let frozen_shard = match bucket {
                Some(bucket) => self.freeze_member(bucket, uuid, shard, buf)?,
                None => self.freeze_shard(&marker.index, uuid, shard, buf)?,
            };
            frozen.push(frozen_shard);
        }
        // ALLOC: the finished view, once a capture.
        let view = PointInTime {
            shards: frozen,
            capturing: false,
            ..marker.clone()
        };
        self.finish(&view, buf)?;
        Ok(view)
    }
}

impl PointInTimeStore {
    /// One shard's own commit register, read and frozen.
    fn freeze_shard(
        &self,
        index: &str,
        uuid: &str,
        shard: u32,
        buf: &mut [u8],
    ) -> Result<PitShard, CaptureError> {
        let at = ShardRef { index, uuid, shard };
        let key = at.manifest_key(&self.base).map_err(MetaError::Format)?;
        let Some(read) = self
            .store
            .read_register(&key, buf)
            .map_err(MetaError::Blob)?
        else {
            return Err(CaptureError::NothingPublished(shard));
        };
        let manifest = CommitManifest::parse(buf.get(..read.len).unwrap_or_default())
            .map_err(MetaError::Format)?;
        Ok(pit_shard(shard, &manifest))
    }

    /// A bucketed member's commit, from its bucket's manifest register: the
    /// one GET a shard's register would cost. Its files are in node packs or
    /// in the shard's own path, as for any shard.
    fn freeze_member(
        &self,
        bucket: &str,
        uuid: &str,
        shard: u32,
        buf: &mut [u8],
    ) -> Result<PitShard, CaptureError> {
        let key = bucket_manifest_key(&self.base, bucket).map_err(MetaError::Format)?;
        let Some(read) = self
            .store
            .read_register(&key, buf)
            .map_err(MetaError::Blob)?
        else {
            return Err(CaptureError::NothingPublished(shard));
        };
        // ALLOC: the member's name and commit, at most the register's size, once a capture.
        let entry = parse_bucket_member(buf.get(..read.len).unwrap_or_default(), uuid)
            .map_err(MetaError::Format)?;
        // The member moved to another bucket or out of one: the index is not as resolved.
        let Some(member) = entry.member.filter(|_| entry.bucket == bucket) else {
            return Err(CaptureError::Replaced);
        };
        let Some(commit) = member.commit else {
            return Err(CaptureError::NothingPublished(shard));
        };
        let manifest = CommitManifest::parse(&commit).map_err(MetaError::Format)?;
        Ok(pit_shard(shard, &manifest))
    }
}

fn pit_shard(shard: u32, manifest: &CommitManifest<'_>) -> PitShard {
    PitShard {
        shard,
        term: manifest.term,
        files: manifest
            .files()
            .iter()
            .map(|file| PitFile {
                // ALLOC: one entry a file, at most MAX_FILES.
                name: file.name.to_string(),
                location: file.location.encode(),
                length: file.length,
            })
            // ALLOC: the entries counted above.
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_format::looks_like_an_id;

    #[test]
    fn each_refusal_says_what_it_is() {
        assert_eq!(CaptureError::NoSuchIndex.to_string(), "no such index");
        assert!(CaptureError::IsAlias.to_string().contains("alias"));
        assert_eq!(CaptureError::Unsupported("why").to_string(), "why");
        assert!(
            CaptureError::NothingPublished(3)
                .to_string()
                .contains("shard 3")
        );
        assert!(CaptureError::Replaced.to_string().contains("recreated"));
        assert_eq!(
            CaptureError::Meta(MetaError::Blob(BlobError::NotFound)).to_string(),
            MetaError::Blob(BlobError::NotFound).to_string()
        );
    }

    #[test]
    fn an_id_is_base64url_of_its_bytes_without_padding() {
        let mut bytes = [0u8; 16];
        for (at, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::try_from(at).unwrap();
        }
        assert_eq!(encode_id(&bytes), "AAECAwQFBgcICQoLDA0ODw");
        assert_eq!(encode_id(&[0xff; 16]), "_____________________w");
        assert_eq!(encode_id(&[0xfb; 16]), "-_v7-_v7-_v7-_v7-_v7-w");
    }

    #[test]
    fn new_ids_look_like_ids_and_differ() {
        let ids: Vec<String> = (0..64).map(|_| new_id().unwrap()).collect();
        assert!(ids.iter().all(|id| looks_like_an_id(id)));
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}
