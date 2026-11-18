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

use super::{BuildError, Overlay};

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
    pub fn new(commit: &DirectoryReader, hidden: &[(usize, i32)]) -> Self {
        let segments = commit.segment_readers();
        // ALLOC: one entry a commit segment.
        let mut live: Vec<Option<FixedBitSet>> = segments.iter().map(|_| None).collect();
        for &(segment, doc) in hidden {
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
