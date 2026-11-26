//! The commit's segments and the overlay's, as one list for the searcher.
//!
//! A mask is a copy of a commit segment's live documents with the hidden
//! documents cleared; the commit's files and reader are never changed. The
//! overlay's segment follows the last commit document.

use std::collections::HashMap;

use lucene_search::directory_reader::{DirectoryReader, OpenedSegments};
use lucene_search::field_norms::FieldNorms;
use lucene_search::multi_segment::OpenSegment;
use lucene_util::fixed_bit_set::FixedBitSet;

use std::sync::Arc;

use super::{BuildError, Copies, Overlay};
use crate::OpenShard;

/// The live documents of the commit's segments with the overlay's hidden
/// documents cleared; `None` for a segment with nothing hidden.
#[derive(Debug, Default)]
pub struct Masks {
    live: Vec<Option<FixedBitSet>>,
}

impl Masks {
    /// Masks over `commit`'s segments hiding `hidden` as
    /// `(segment, document)`.
    #[must_use]
    pub fn new(commit: &DirectoryReader, hidden: impl IntoIterator<Item = (usize, i32)>) -> Self {
        let segments = commit.segment_readers();
        // ALLOC: one entry a commit segment.
        let mut live: Vec<Option<FixedBitSet>> = segments.iter().map(|_| None).collect();
        for (segment, doc) in hidden {
            let (Some(slot), Some(reader)) = (live.get_mut(segment), segments.get(segment)) else {
                continue;
            };
            let Ok(index) = usize::try_from(doc) else {
                continue;
            };
            let bits = slot.get_or_insert_with(|| {
                // ALLOC: one bit set a segment with a hidden document, sized by its max_doc.
                reader.live_docs().cloned().unwrap_or_else(|| {
                    let len = usize::try_from(reader.max_doc).unwrap_or(0);
                    let mut all = FixedBitSet::new(len);
                    all.set_range(0, len);
                    all
                })
            });
            if index < bits.len() {
                bits.clear(index);
            }
        }
        Self { live }
    }
}

impl Masks {
    /// The masks as one optional bit set a commit segment.
    #[must_use]
    pub fn into_live(self) -> Vec<Option<FixedBitSet>> {
        self.live
    }
}

/// The commit's open segments, each with its mask where it has one,
/// followed by the overlay's segment numbered after the commit's last
/// document (`commit_max_doc`).
#[must_use]
pub fn combine<'a>(
    commit: &'a OpenedSegments<'a>,
    masks: &'a Masks,
    overlay: Option<&'a OpenedSegments<'a>>,
    commit_max_doc: i32,
) -> Vec<OpenSegment<'a>> {
    let mut segments = commit.as_open_segments();
    for (segment, mask) in segments.iter_mut().zip(&masks.live) {
        if let Some(bits) = mask {
            segment.live_docs = Some(bits);
        }
    }
    if let Some(overlay) = overlay {
        for mut segment in overlay.as_open_segments() {
            segment.doc_base = segment.doc_base.saturating_add(commit_max_doc);
            segments.push(segment);
        }
    }
    segments
}

impl Overlay {
    /// The overlay segment's norms for `fields`, to follow the commit's in
    /// the searcher's norms list; empty for an overlay with no document.
    ///
    /// # Errors
    ///
    /// [`BuildError::Engine`] when the segment's norms cannot be read.
    pub fn norms(
        &self,
        fields: &[String],
    ) -> Result<Vec<HashMap<String, FieldNorms<'_>>>, BuildError> {
        match self.reader() {
            Some(reader) => reader
                .field_norms_by_field(fields)
                // ALLOC: one message on a failed read.
                .map_err(|e| BuildError::Engine(e.to_string())),
            None => Ok(Vec::new()),
        }
    }
}

impl Overlay {
    /// `commit`'s segments with the documents this overlay replaces or
    /// deletes hidden, followed by the overlay's segment: one reader a search
    /// takes as it takes the commit's. `commit` is not changed.
    ///
    /// # Errors
    ///
    /// [`BuildError::Engine`] when the engine refuses the combined segments.
    pub fn view_of(
        &self,
        commit: &DirectoryReader,
        copies: &Copies,
    ) -> Result<DirectoryReader, BuildError> {
        // ALLOC: one message on a failed combine.
        let engine = |e: lucene_search::directory_reader::Error| BuildError::Engine(e.to_string());
        // ALLOC: one reader handle (reference counts) an overlay segment.
        let extra = self
            .reader()
            .map(|r| r.segment_readers().to_vec())
            .unwrap_or_default();
        let extra_len = extra.len();
        let combined = commit.with_readers_appended(extra).map_err(engine)?;
        let mut live = Masks::new(commit, self.hidden(copies)).into_live();
        // ALLOC: one entry an overlay segment.
        live.extend((0..extra_len).map(|_| None));
        combined.with_live_docs_masks(live).map_err(engine)
    }
}

impl<S: oss_blob::BlobStore + 'static> OpenShard<S> {
    /// This shard as a search sees it with `overlay` on top of its commit:
    /// the same directory and descriptor, and a reader over the commit's
    /// segments without the documents the overlay replaces, then the
    /// overlay's. The shard itself is not changed.
    ///
    /// # Errors
    ///
    /// As [`Overlay::view_of`].
    pub fn with_overlay(&self, overlay: &Overlay, copies: &Copies) -> Result<Self, BuildError> {
        Ok(Self {
            directory: Arc::clone(&self.directory),
            reader: overlay.view_of(&self.reader, copies)?,
            meta: self.meta.clone(),
            shard: self.shard,
        })
    }
}
