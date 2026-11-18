//! Where the commit holds the documents an overlay replaces or deletes.
//!
//! One filter search of the `_id` terms over the commit's live documents,
//! sorted on `_seq_no`, then each hit's stored `_id`. It reads only through
//! the reader it is given, so it adds nothing beyond that reader's own lazy
//! reads; it runs in the background and never inside a search.

use std::collections::HashMap;

use lucene_codecs::stored_fields::{DocumentVisitor, FieldValue};
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::query::TermInSetQuery;
use lucene_search::top_field::{SortField, SortType};
use lucene_search::{BooleanQuery, Clause};
use oss_doc::lucene::uid;

use super::{BuildError, MAX_IDS};

/// One live document of the commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Copy {
    /// The commit's segment, in the order its reader lists them.
    pub segment: usize,
    /// The document's number in that segment.
    pub doc: i32,
    /// Its sequence number.
    pub seq_no: i64,
}

/// The commit's live copies of some ids.
#[derive(Debug, Default)]
pub struct Copies {
    found: HashMap<String, Copy>,
}

impl Copies {
    /// The commit's copy of `id`, if it holds one.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Copy> {
        self.found.get(id)
    }

    /// How many ids have a copy.
    #[must_use]
    pub fn len(&self) -> usize {
        self.found.len()
    }

    /// Whether no id has a copy.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }
}

fn engine(e: impl std::fmt::Display) -> BuildError {
    // ALLOC: one message on a failed lookup.
    BuildError::Engine(e.to_string())
}

/// The commit's live copy of each of `ids` it holds.
///
/// # Errors
///
/// [`BuildError::TooManyIds`] past [`MAX_IDS`] ids, [`BuildError::Record`]
/// for an id that cannot be encoded, and [`BuildError::Engine`] when the
/// commit cannot be read.
pub fn find_copies(reader: &DirectoryReader, ids: &[String]) -> Result<Copies, BuildError> {
    if ids.len() > MAX_IDS {
        return Err(BuildError::TooManyIds);
    }
    if ids.is_empty() {
        return Ok(Copies::default());
    }
    // ALLOC: one term and one entry an id, at most MAX_IDS.
    let mut by_term: HashMap<Vec<u8>, &str> = HashMap::new();
    for id in ids {
        let term = uid(id).map_err(|e| BuildError::Record {
            // ALLOC: one id and message on a failed lookup.
            id: id.clone(),
            reason: format!("{e:?}"),
        })?;
        by_term.insert(term, id);
    }
    // ALLOC: one term an id, at most MAX_IDS.
    let terms: Vec<Vec<u8>> = by_term.keys().cloned().collect();
    // ALLOC: one query a lookup, with the terms above.
    let query = BooleanQuery {
        filter: vec![Clause::TermInSet(TermInSetQuery::new("_id", terms))],
        ..BooleanQuery::default()
    };
    let opened = reader.open_segments().map_err(engine)?;
    let segments = opened.as_open_segments();
    // ALLOC: one entry a segment.
    let norms: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = vec![None; segments.len()];
    let searcher = IndexSearcher::new(&segments, &norms).map_err(engine)?;
    let sort = [SortField::numeric("_seq_no", SortType::Long, false)];
    let top = searcher
        .search_sorted(reader.segment_readers(), &query, ids.len(), &sort, None)
        .map_err(engine)?;
    // ALLOC: one entry an id, at most MAX_IDS.
    let mut found = HashMap::new();
    for hit in &top.hits {
        let Some(segment) = searcher.segment_of(hit.doc) else {
            continue;
        };
        let (Some(open), Some(seg_reader)) =
            (segments.get(segment), reader.segment_readers().get(segment))
        else {
            continue;
        };
        let doc = hit.doc.saturating_sub(open.doc_base);
        let Some(number) = seg_reader
            .field_infos()
            .field_by_name("_id")
            .map(|f| f.number)
        else {
            continue;
        };
        let mut visitor = DocumentVisitor::for_fields(&[number]);
        if !seg_reader
            .visit_stored_document(doc, &mut visitor)
            .map_err(engine)?
        {
            continue;
        }
        let stored = visitor.into_document();
        let Some(FieldValue::Binary(term)) = stored.fields.into_iter().next().map(|f| f.value)
        else {
            continue;
        };
        if let (Some(id), Some(seq_no)) = (by_term.get(&term), hit.values.first()) {
            found.insert(
                // ALLOC: one id a found copy, at most MAX_IDS.
                (*id).to_owned(),
                Copy {
                    segment,
                    doc,
                    seq_no: *seq_no,
                },
            );
        }
    }
    Ok(Copies { found })
}

/// The commit user data key the writer stores the local checkpoint under.
const LOCAL_CHECKPOINT_KEY: &str = "local_checkpoint";

/// The local checkpoint the commit holds: every operation up to it is in the
/// commit. `-1` (no operation performed) when the commit has none or it is
/// not a number. Reads nothing: the user data is in the commit's own
/// `segments_N`.
#[must_use]
pub fn local_checkpoint(reader: &DirectoryReader) -> i64 {
    reader
        .segment_infos
        .user_data
        .iter()
        .find(|(key, _)| key == LOCAL_CHECKPOINT_KEY)
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(-1)
}

/// The heap the masks of a view over `reader`'s commit take at most: a
/// bit set of each segment's `max_doc` bits, whether or not a document of it
/// is hidden. Charged with an overlay's estimate, since the view a search
/// caches holds them as long as the overlay.
#[must_use]
pub fn mask_bytes(reader: &DirectoryReader) -> usize {
    reader.segment_readers().iter().fold(0_usize, |bytes, r| {
        let docs = usize::try_from(r.max_doc).unwrap_or(0);
        bytes.saturating_add(docs.div_ceil(64).saturating_mul(8))
    })
}
