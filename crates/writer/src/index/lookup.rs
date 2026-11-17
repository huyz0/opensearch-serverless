//! The current version of an id in a shard's segments, as
//! `VersionsAndSeqNoResolver.loadDocIdAndSeqNo` finds it: the live document
//! with that `_id` term, its `_version`, `_seq_no` and `_primary_term`.

use lucene_search::directory_reader::DirectoryReader;
use lucene_search::reader::{LeafReader, NO_MORE_DOCS, PostingsFlags};

use super::plan::Current;

/// The engine's error as a message.
fn fail(e: &dyn std::fmt::Display) -> String {
    // ALLOC: error path; one message.
    e.to_string()
}

/// The newest live document whose `_id` is `uid`, from the newest segment
/// back; soft-deleted documents and tombstones are not live in `reader`.
///
/// # Errors
///
/// A segment that does not read.
pub fn find(reader: &DirectoryReader, uid: &[u8]) -> Result<Option<Current>, String> {
    for segment in reader.segment_readers().iter().rev() {
        let Some(terms) = segment.terms("_id").map_err(|e| fail(&e))? else {
            continue;
        };
        let mut te = terms.iterator().map_err(|e| fail(&e))?;
        if !te.try_seek_exact(uid).map_err(|e| fail(&e))? {
            continue;
        }
        let mut postings = te.postings(PostingsFlags::None).map_err(|e| fail(&e))?;
        let live = segment.live_docs();
        let mut found = None;
        loop {
            let doc = postings.next_doc().map_err(|e| fail(&e))?;
            if doc == NO_MORE_DOCS {
                break;
            }
            if live.is_none_or(|bits| bits.get_doc(doc)) {
                found = Some(doc);
            }
        }
        let Some(doc) = found else {
            continue;
        };
        let value = |field: &str| -> Result<i64, String> {
            let mut dv = segment
                .numeric_doc_values(field)
                .map_err(|e| fail(&e))?
                .ok_or_else(|| fail(&format_args!("no {field} doc values")))?;
            if dv.advance_exact(doc).map_err(|e| fail(&e))? {
                Ok(dv.long_value())
            } else {
                Err(fail(&format_args!("no {field} for document {doc}")))
            }
        };
        return Ok(Some(Current {
            version: value("_version")?,
            seq_no: value("_seq_no")?,
            term: value("_primary_term")?,
            deleted: false,
        }));
    }
    Ok(None)
}
