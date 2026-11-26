//! Putting a commit's fresh files into the store: a file of its own, or a
//! segment's files as one blob.

use std::collections::BTreeMap;

use lucene_store::Directory;
use oss_blob::CreateOutcome;
use oss_format::{FileLocation, encode_segment_header, segment_blob_order};

use super::PublishError;
use super::blob::SegmentBlob;
use super::publisher::ShardPublisher;
use super::register::Known;

/// The buffer a blob's PUT streams through.
const IO_BUF_BYTES: usize = 64 * 1024;

impl ShardPublisher {
    pub(super) fn put_whole(
        &self,
        term_dir: &str,
        name: &str,
        uploaded: &mut BTreeMap<String, Known>,
        entries: &mut BTreeMap<String, Known>,
    ) -> Result<(), PublishError> {
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let key = format!("{}{term_dir}/{name}", self.base);
        self.store
            .put(&key, &mut oss_blob::SliceSource::new(&[]), &mut [0u8; 1])?;
        let known = Known {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            location: term_dir.to_owned(),
            length: 0,
        };
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        uploaded.insert(name.to_owned(), known.clone());
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        entries.insert(name.to_owned(), known);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn put_segment(
        &mut self,
        dir: &dyn Directory,
        term_dir: &str,
        segment: &str,
        plain: &str,
        taken: bool,
        group: &mut [(&str, u64)],
        now_millis: u64,
        uploaded: &mut BTreeMap<String, Known>,
        entries: &mut BTreeMap<String, Known>,
        blobs: &mut Vec<String>,
    ) -> Result<(), PublishError> {
        segment_blob_order(group);
        let header = encode_segment_header(group)?;
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
        let mut source = SegmentBlob::new(header.clone(), parts);
        let mut buf = [0u8; IO_BUF_BYTES];
        // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
        let mut blob = plain.to_owned();
        let mut created = false;
        if !taken {
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            let key = format!("{}{term_dir}/{blob}", self.base);
            created =
                self.store.create_if_absent(&key, &mut source, &mut buf)? == CreateOutcome::Created;
        }
        if !created {
            // An earlier attempt of this term, or a writer that reused the
            // segment's name, left a blob by it.
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            blob = format!("{segment}-{}.seg", self.ids.next(now_millis));
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            let key = format!("{}{term_dir}/{blob}", self.base);
            oss_blob::ChunkSource::rewind(&mut source)?;
            if self.store.create_if_absent(&key, &mut source, &mut buf)? != CreateOutcome::Created {
                return Err(PublishError::Unusable("a fresh blob name was taken"));
            }
        }
        let mut offset = u64::try_from(header.len()).unwrap_or(u64::MAX);
        for (name, length) in group.iter() {
            let known = Known {
                location: FileLocation::Packed {
                    term_dir,
                    blob: &blob,
                    offset,
                    length: *length,
                }
                .encode(),
                length: *length,
            };
            offset = offset.saturating_add(*length);
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            uploaded.insert((*name).to_owned(), known.clone());
            // ALLOC: a publish's files, names and keys: at most manifest::MAX_FILES entries.
            entries.insert((*name).to_owned(), known);
        }
        blobs.push(blob);
        Ok(())
    }
}
