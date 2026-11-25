//! Hits that tie on score ordered by `_id` ascending, for a search whose
//! answer must not depend on the layout of the shard's segments (a forwarded
//! search, whose owner and a Java reader hold the same documents in
//! different segments).
//!
//! The collector gathers [`EXTRA`] documents past the window, so a run of
//! tied scores that crosses the window's end is cut by `_id` and not by
//! document order. A run longer than the extra room is cut by `_id` among the
//! documents gathered: a valid page, not the one a full read would give.

use lucene_codecs::stored_fields::{StoredFieldVisitor, VisitStatus};
use lucene_search::ScoreDoc;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::response::id_text;
use crate::search::ShardError;

/// How many documents past the window the collector gathers.
pub(crate) const EXTRA: usize = 1024;

/// The most bytes of a stored `_id`.
const MAX_ID_BYTES: usize = 512;

/// A document's stored `_id`.
struct IdOnly {
    field: Option<i32>,
    id: Vec<u8>,
}

impl StoredFieldVisitor for IdOnly {
    fn needs_field(&mut self, n: i32) -> lucene_codecs::stored_fields::Result<VisitStatus> {
        Ok(if Some(n) == self.field {
            VisitStatus::Yes
        } else {
            VisitStatus::No
        })
    }

    fn binary_field(&mut self, n: i32, value: &[u8]) -> lucene_codecs::stored_fields::Result<()> {
        if Some(n) == self.field {
            self.id.clear();
            self.id
                .extend_from_slice(value.get(..MAX_ID_BYTES).unwrap_or(value));
        }
        Ok(())
    }
}

fn id_of<S: BlobStore + 'static>(shard: &OpenShard<S>, doc: i32) -> Result<Vec<u8>, ShardError> {
    let segment = shard
        .reader
        .segment_readers()
        .iter()
        .rev()
        .find(|r| r.doc_base <= doc)
        .ok_or_else(|| ShardError::engine("a hit outside every segment"))?;
    let mut visitor = IdOnly {
        field: segment.field_infos().field_by_name("_id").map(|f| f.number),
        // ALLOC: one id, at most MAX_ID_BYTES.
        id: Vec::new(),
    };
    segment
        .visit_source_document(doc.saturating_sub(segment.doc_base), &mut visitor)
        .map_err(ShardError::engine)?;
    Ok(id_text(&visitor.id))
}

/// `docs` (by score, then document order) with each run of equal scores
/// that reaches into the first `window` ordered by `_id` ascending, cut to
/// `window`.
pub(crate) fn order<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    docs: &[ScoreDoc],
    window: usize,
) -> Result<Vec<ScoreDoc>, ShardError> {
    // ALLOC: one entry a gathered document, at most window + EXTRA.
    let mut out = Vec::with_capacity(docs.len());
    let mut at = 0;
    while at < docs.len() && at < window {
        let end = docs[at..]
            .iter()
            .position(|d| d.score.to_bits() != docs[at].score.to_bits())
            .map_or(docs.len(), |n| at.saturating_add(n));
        let run = &docs[at..end];
        if run.len() > 1 {
            // ALLOC: one id a document of the run, at most MAX_ID_BYTES each.
            let mut keyed: Vec<(Vec<u8>, ScoreDoc)> = run
                .iter()
                .map(|d| id_of(shard, d.doc_id).map(|id| (id, *d)))
                .collect::<Result<_, _>>()?;
            keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.doc_id.cmp(&b.1.doc_id)));
            out.extend(keyed.into_iter().map(|(_, d)| d));
        } else {
            out.extend_from_slice(run);
        }
        at = end;
    }
    out.truncate(window);
    Ok(out)
}
