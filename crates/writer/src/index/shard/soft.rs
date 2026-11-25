//! The soft-delete update of a replaced or deleted document.

use lucene_index::buffered_updates::{DocValuesUpdate, Term};

/// `Lucene.newSoftDeletesField()` as an update of every document with `_id`
/// `uid`.
pub(super) fn soft_delete(uid: &[u8]) -> DocValuesUpdate {
    DocValuesUpdate::Numeric {
        // ALLOC: one term a replaced document.
        term: Term::new("_id", uid.to_vec()),
        field: "__soft_deletes".to_owned(),
        value: Some(1),
    }
}
