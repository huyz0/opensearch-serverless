//! A shard's small segment blobs sent to the node's packs instead of blobs
//! of their own, when a publish asks for it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use lucene_store::Directory;
use oss_format::{encode_segment_header, segment_blob_order};

use super::PublishError;
use super::blob::SegmentBlob;
use super::publisher::ShardPublisher;
use super::register::Known;
use crate::pack::{PackWriter, Part};

/// Where a shard's small segment blobs go when its publishes use node packs:
/// the node's pack writer, and what each pack's header records of the shard.
#[derive(Clone)]
pub struct PackPlacement {
    /// The node's pack writer, shared by every shard it publishes.
    pub writer: Arc<PackWriter>,
    /// The shard's index name, which collection needs to find its manifest.
    pub index_name: String,
    /// The shard's index uuid.
    pub index_uuid: String,
    /// The shard.
    pub shard: u32,
    /// The largest segment blob that goes into a pack; larger ones keep a
    /// blob of their own, so no shard's big merged segment sits in a pack
    /// beside other shards' small ones.
    pub max_blob_bytes: u64,
}

impl std::fmt::Debug for PackPlacement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackPlacement")
            .field("index", &self.index_name)
            .field("shard", &self.shard)
            .field("max_blob_bytes", &self.max_blob_bytes)
            .finish_non_exhaustive()
    }
}

/// A segment blob bound for a pack, before the pack is written.
pub(super) struct Packable {
    header_len: u64,
    files: Vec<(String, u64)>,
}

impl ShardPublisher {
    /// The segment's blob as a pack part when this publish packs and the
    /// blob is small enough; `None` leaves it to a blob of its own.
    pub(super) fn pack_candidate(
        &mut self,
        dir: &dyn Directory,
        segment: &str,
        plain: &str,
        taken: bool,
        group: &mut [(&str, u64)],
        now_millis: u64,
    ) -> Result<Option<(Packable, Part)>, PublishError> {
        let Some(placement) = &self.packs else {
            return Ok(None);
        };
        segment_blob_order(group);
        let header = encode_segment_header(group)?;
        let header_len = u64::try_from(header.len()).unwrap_or(u64::MAX);
        let total = group
            .iter()
            .fold(header_len, |sum, (_, length)| sum.saturating_add(*length));
        if !fits(total, placement.max_blob_bytes) {
            return Ok(None);
        }
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut parts = Vec::with_capacity(group.len());
        for (name, length) in group.iter() {
            let input = dir
                .open(name)
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                .map_err(|e| PublishError::Directory(format!("{name}: {e}")))?;
            if u64::try_from(input.len()).ok() != Some(*length) {
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                return Err(PublishError::Directory(format!(
                    "{name} changed length while it was being published"
                )));
            }
            parts.push(input);
        }
        // The block's name is informational (collection reads the header, a
        // manifest names ranges); keep it distinct for a reused segment name.
        let blob = if taken {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            format!("{segment}-{}.seg", self.ids.next(now_millis))
        } else {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            plain.to_owned()
        };
        let part = Part {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            index_uuid: placement.index_uuid.clone(),
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            index_name: placement.index_name.clone(),
            shard: placement.shard,
            name: blob,
            // ALLOC: the part's source box, one a segment of a publish.
            source: Box::new(SegmentBlob::new(header, parts)),
        };
        let files = group
            .iter()
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            .map(|(name, length)| ((*name).to_owned(), *length))
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            .collect();
        Ok(Some((Packable { header_len, files }, part)))
    }

    /// Writes a publish's small segment blobs as one pack, all in the same
    /// one, and records where each file landed.
    pub(super) fn put_packed(
        &self,
        packable: Vec<(Packable, Part)>,
        packed_at: &mut Option<Instant>,
        uploaded: &mut BTreeMap<String, Known>,
        entries: &mut BTreeMap<String, Known>,
    ) -> Result<(), PublishError> {
        let Some(placement) = &self.packs else {
            return Ok(());
        };
        if packable.is_empty() {
            return Ok(());
        }
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let (laid, parts): (Vec<Packable>, Vec<Part>) = packable.into_iter().unzip();
        // Taken before the pack is asked for, so the pack is never younger
        // than this says.
        packed_at.get_or_insert_with(Instant::now);
        let landed = placement.writer.add(parts)?;
        for (found, at) in laid.iter().zip(&landed) {
            let mut offset = found.header_len;
            for (name, length) in &found.files {
                let Some(location) = at.file_at(offset, *length) else {
                    return Err(PublishError::Unusable("a file lies outside its pack block"));
                };
                let known = Known {
                    location: location.encode(),
                    length: *length,
                };
                offset = offset.saturating_add(*length);
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                uploaded.insert(name.clone(), known.clone());
                // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
                entries.insert(name.clone(), known);
            }
        }
        Ok(())
    }
}

/// Whether a segment's blob of `total` bytes may go into a pack: one of
/// exactly `max` bytes may.
fn fits(total: u64, max: u64) -> bool {
    total <= max
}

#[cfg(test)]
mod tests {
    use super::fits;

    #[test]
    fn a_blob_of_exactly_the_most_fits_and_one_byte_over_does_not() {
        assert!(fits(9, 10));
        assert!(fits(10, 10));
        assert!(!fits(11, 10));
    }
}
