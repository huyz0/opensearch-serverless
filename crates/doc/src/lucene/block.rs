//! A document with nested objects as a block of Lucene documents.

use lucene_index::index_writer::{DocumentVector, ExplicitDocument};

use crate::document::{NestedDocument, ParsedDocument};
use crate::mapping::{Mapping, MappingError};

use super::builder::Builder;
use super::encode::encode_id;
use super::fields::MetaField;
use super::{FieldNumbers, Indexer, Meta};

impl Indexer {
    /// One nested object as the Lucene document `nestedContext` makes: its
    /// fields, `_id` (indexed, not stored, so deleting the root's `_id`
    /// deletes it) and `_nested_path`.
    fn child(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        mapping: &Mapping,
        nested: &NestedDocument,
        id: &[u8],
    ) -> Result<ExplicitDocument, MappingError> {
        // ALLOC: one nested document's Lucene fields, bounded by the document's bytes (the HTTP body bound).
        let mut out = Builder::default();
        let n = self.meta(numbers, MetaField::Id)?;
        out.term(n, id);
        self.add_fields(numbers, &mut out, mapping, &nested.fields)?;
        let n = self.meta(numbers, MetaField::NestedPath)?;
        out.term(n, nested.path.as_bytes());
        Ok(out.finish().0)
    }

    /// The Lucene documents of `doc` as a block: each nested object's
    /// document, in the order they completed, then the document itself,
    /// with the document's vectors.
    ///
    /// # Errors
    ///
    /// As [`Indexer::document`].
    pub fn block_with_vectors(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        mapping: &Mapping,
        doc: &ParsedDocument,
        meta: &Meta<'_>,
        routing: Option<&str>,
    ) -> Result<(Vec<ExplicitDocument>, Vec<Vec<DocumentVector>>), MappingError> {
        let id = encode_id(meta.id)?;
        // ALLOC: one document a nested object and the document, bounded by the nested objects limit.
        let mut docs = Vec::with_capacity(doc.nested.len().saturating_add(1));
        // ALLOC: as above.
        let mut vectors = Vec::with_capacity(doc.nested.len().saturating_add(1));
        for nested in &doc.nested {
            docs.push(self.child(numbers, mapping, nested, &id)?);
            // ALLOC: an empty list, which holds nothing.
            vectors.push(Vec::new());
        }
        let (root, root_vectors) =
            self.document_with_vectors(numbers, mapping, doc, meta, routing)?;
        docs.push(root);
        vectors.push(root_vectors);
        Ok((docs, vectors))
    }

    /// As [`Indexer::block_with_vectors`], for a caller that hands the
    /// engine no vectors.
    ///
    /// # Errors
    ///
    /// As [`Indexer::document_routed`].
    pub fn block_routed(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        mapping: &Mapping,
        doc: &ParsedDocument,
        meta: &Meta<'_>,
        routing: Option<&str>,
    ) -> Result<Vec<ExplicitDocument>, MappingError> {
        let (docs, vectors) = self.block_with_vectors(numbers, mapping, doc, meta, routing)?;
        if vectors.iter().any(|v| !v.is_empty()) {
            return Err(MappingError::unsupported(
                "a document with a knn_vector value for a caller that does not index vectors",
            ));
        }
        Ok(docs)
    }
}
