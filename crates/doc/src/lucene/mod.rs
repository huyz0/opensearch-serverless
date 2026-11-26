//! A parsed document as the Lucene document OpenSearch indexes for it: the
//! metadata fields, then each value's fields as its mapper's
//! `parseCreateField` adds them, then `_ignored`; each field registered
//! with the engine, which numbers it as `FieldInfos.FieldNumbers` does.
//!
//! Ported from the field mappers' `parseCreateField`, `IdFieldMapper`,
//! `SeqNoFieldMapper`, `VersionFieldMapper`, `SourceFieldMapper`,
//! `FieldNamesFieldMapper` and `IgnoredFieldMapper` (server, OpenSearch
//! 3.10), and `IndexingChain.invert`'s positions, offsets and lengths.

mod builder;
pub mod derived;
mod encode;
pub(crate) mod fields;
pub(crate) mod invert;
mod parts;

use std::collections::HashMap;
use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::FieldInfo;
use lucene_codecs::per_field_knn_vectors::KnnVectorsFormat;
use lucene_codecs::stored_fields::{FieldValue as Stored, StoredField};
use lucene_index::index_writer::{
    DocumentVector, Error as EngineError, ExplicitDocument, IndexWriter,
};
use oss_ext::analyzer::Analyze;

use crate::document::{FieldValue, NestedDocument, ParsedDocument, hash_order};
use crate::mapping::{Mapping, MappingError, Num, Value};

use builder::Builder;
use encode::{double_to_sortable_long, encode_id, float_to_sortable_int, int_point, long_point};
pub(crate) use fields::{MetaField, Shape, Spec};
use parts::leaf;
pub use parts::{Meta, uid};

/// Where a field's number comes from: the engine's field numbers, which
/// hold for every segment of an index writer and refuse a field whose
/// schema changes (`FieldInfos.FieldNumbers.addOrGet` and
/// `verifySameSchema`).
pub trait FieldNumbers {
    /// The number of the field `info` describes, registering it on first
    /// sight; `info.number` is ignored.
    ///
    /// # Errors
    ///
    /// The engine's message when the field's schema differs from the one
    /// it was registered with.
    fn number(&mut self, info: FieldInfo) -> Result<i32, String>;

    /// Routes the vector field `name` to an HNSW graph of `m` connections a
    /// node and `beam_width`, as `PerFieldKnnVectorsFormat` does.
    fn vector_graph(&mut self, _name: &str, _m: i32, _beam_width: i32) {}
}

impl FieldNumbers for IndexWriter<'_> {
    fn number(&mut self, info: FieldInfo) -> Result<i32, String> {
        self.register_field(info).map_err(|e| match e {
            EngineError::Explicit(message) => message,
            // ALLOC: error path; one message.
            other => other.to_string(),
        })
    }

    fn vector_graph(&mut self, name: &str, m: i32, beam_width: i32) {
        self.set_knn_vectors_format_for_field(
            name,
            KnnVectorsFormat::Hnsw {
                max_conn: m,
                beam_width,
            },
        );
    }
}

/// A field the engine has numbered and the spec it was registered with.
#[derive(Clone, Copy, Debug)]
struct Slot {
    number: i32,
    spec: Option<Spec>,
}

/// Builds an index writer's Lucene documents: the analyzer, and the
/// numbers the engine gave the fields seen so far. One lives as long as
/// its writer.
pub struct Indexer {
    analyzer: Analyzer,
    slots: HashMap<String, Slot>,
    /// Whether a document's `_source` is stored with its vectors masked, for
    /// a writer whose segments list the vector fields as derived.
    derived_source: bool,
    /// The pinned analyzers of this index by the name a mapping gives them.
    modules: HashMap<String, Arc<dyn Analyze>>,
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}

impl Indexer {
    /// An indexer that has seen no field yet.
    pub fn new() -> Self {
        Indexer {
            // The `standard` analyzer as OpenSearch builds it: no stop words.
            analyzer: Analyzer::standard(None),
            // ALLOC: one entry a field of the index, at most the mapping's total fields limit plus the metadata fields.
            slots: HashMap::new(),
            derived_source: false,
            // ALLOC: one entry a pin, at most four.
            modules: HashMap::new(),
        }
    }

    /// Analyzes a field whose mapping names `name` with `analyzer`, the
    /// module a pin loaded, in place of the built-in analyzer.
    #[must_use]
    pub fn with_module(mut self, name: &str, analyzer: Arc<dyn Analyze>) -> Self {
        // ALLOC: one entry a pin.
        self.modules.insert(name.to_owned(), analyzer);
        self
    }

    /// Stores a document's `_source` with each vector's value replaced by
    /// `1` ([`derived::mask_vectors`]) when `on`. Only for a writer whose
    /// segments list the vector fields under `derived_vector_fields` and
    /// record the k-NN codec, or a reader would return the `1`.
    #[must_use]
    pub fn with_derived_source(mut self, on: bool) -> Self {
        self.derived_source = on;
        self
    }

    /// Every field numbered so far, with the spec it was registered with (`None` for a metadata
    /// field): what the fast ingest plan may name without changing the engine's numbering.
    pub(crate) fn numbered(&self) -> impl Iterator<Item = (&str, i32, Option<Spec>)> {
        self.slots
            .iter()
            .map(|(n, s)| (n.as_str(), s.number, s.spec))
    }

    /// How many fields have been numbered, which only grows.
    pub(crate) fn numbered_len(&self) -> usize {
        self.slots.len()
    }

    /// The number of the metadata field `field`, once a document has registered it.
    pub(crate) fn meta_number(&self, field: MetaField) -> Option<i32> {
        self.slots.get(field.name()).map(|s| s.number)
    }

    /// The number of `name`, registering it with `numbers` the first time
    /// and again whenever its spec changes, so that the engine refuses a
    /// mapping update its schema cannot take, as Lucene refuses it.
    fn slot(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        name: &str,
        spec: Option<Spec>,
        meta: Option<MetaField>,
    ) -> Result<i32, MappingError> {
        if let Some(s) = self.slots.get(name)
            && s.spec == spec
        {
            return Ok(s.number);
        }
        let info = match (spec, meta) {
            (Some(s), _) => s.field_info(name, 0),
            (None, Some(m)) => m.field_info(0),
            (None, None) => FieldInfo::new(name, 0),
        };
        let number = numbers
            .number(info)
            .map_err(|message| MappingError::illegal_argument(&message))?;
        if let Some(v) = spec.and_then(|s| s.vector) {
            numbers.vector_graph(name, v.m, v.beam_width);
        }
        // ALLOC: one entry a new field, bounded as `slots`.
        self.slots.insert(name.to_owned(), Slot { number, spec });
        Ok(number)
    }

    fn meta(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        field: MetaField,
    ) -> Result<i32, MappingError> {
        self.slot(numbers, field.name(), None, Some(field))
    }

    /// The number of the soft-deletes field, registering it the first
    /// time, as `IndexWriter.softUpdateDocument` registers it before the
    /// document it adds.
    ///
    /// # Errors
    ///
    /// The engine refusing the field.
    pub fn soft_deletes(&mut self, numbers: &mut dyn FieldNumbers) -> Result<i32, MappingError> {
        self.meta(numbers, MetaField::SoftDeletes)
    }

    /// The tombstone a delete writes (`createDeleteTombstoneDoc`): `_id`,
    /// `_seq_no`, `_primary_term`, `_version` and `_tombstone`, with the
    /// soft-deletes field the engine adds to it.
    ///
    /// # Errors
    ///
    /// An empty id, or the engine refusing a field.
    pub fn tombstone(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        id: &str,
        seq_no: i64,
        primary_term: i64,
        version: i64,
    ) -> Result<ExplicitDocument, MappingError> {
        // ALLOC: one tombstone's fields, a few bytes and the id.
        let mut out = Builder::default();
        let uid = encode_id(id)?;
        let n = self.meta(numbers, MetaField::Id)?;
        out.term(n, &uid);
        out.stored(n, Stored::Binary(uid));
        let n = self.meta(numbers, MetaField::SeqNo)?;
        out.point(n, long_point(seq_no));
        out.doc_value(n, seq_no);
        let n = self.meta(numbers, MetaField::PrimaryTerm)?;
        out.doc_value(n, primary_term);
        let n = self.meta(numbers, MetaField::Version)?;
        out.doc_value(n, version);
        let n = self.meta(numbers, MetaField::Tombstone)?;
        out.doc_value(n, 1);
        let n = self.meta(numbers, MetaField::SoftDeletes)?;
        out.doc_value(n, 1);
        Ok(out.finish().0)
    }

    /// The Lucene document for `doc`, read against `mapping` (the mapping
    /// grown by `doc`, when it grew one).
    ///
    /// # Errors
    ///
    /// An empty id, a field the mapping does not map, or a field whose
    /// schema the engine refuses to change.
    pub fn document(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        mapping: &Mapping,
        doc: &ParsedDocument,
        meta: &Meta<'_>,
    ) -> Result<ExplicitDocument, MappingError> {
        self.document_routed(numbers, mapping, doc, meta, None)
    }

    /// As [`Indexer::document`], for a document indexed with a custom
    /// routing value: `_routing` is one term and the stored string
    /// (`RoutingFieldMapper.preParse`).
    ///
    /// # Errors
    ///
    /// As [`Indexer::document`].
    pub fn document_routed(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        mapping: &Mapping,
        doc: &ParsedDocument,
        meta: &Meta<'_>,
        routing: Option<&str>,
    ) -> Result<ExplicitDocument, MappingError> {
        let (document, vectors) =
            self.document_with_vectors(numbers, mapping, doc, meta, routing)?;
        if vectors.is_empty() {
            Ok(document)
        } else {
            // A caller that hands the engine no vectors would lose them.
            Err(MappingError::unsupported(
                "a document with a knn_vector value for a caller that does not index vectors",
            ))
        }
    }

    /// As [`Indexer::document_routed`], with the document's vectors, one
    /// for each `knn_vector` field it has a value for, to be handed to the
    /// engine beside it.
    ///
    /// # Errors
    ///
    /// As [`Indexer::document`].
    pub fn document_with_vectors(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        mapping: &Mapping,
        doc: &ParsedDocument,
        meta: &Meta<'_>,
        routing: Option<&str>,
    ) -> Result<(ExplicitDocument, Vec<DocumentVector>), MappingError> {
        // ALLOC: one document's Lucene fields, bounded by the document's bytes (the HTTP body bound).
        let mut out = Builder::default();
        let id = encode_id(meta.id)?;
        let n = self.meta(numbers, MetaField::Id)?;
        out.term(n, &id);
        out.stored(n, Stored::Binary(id));
        if let Some(routing) = routing {
            let n = self.meta(numbers, MetaField::Routing)?;
            out.term(n, routing.as_bytes());
            // ALLOC: the routing value, one a routed document; bounded by the request's bytes.
            out.stored(n, Stored::String(routing.to_owned()));
        }
        let n = self.meta(numbers, MetaField::SeqNo)?;
        out.point(n, long_point(meta.seq_no));
        out.doc_value(n, meta.seq_no);
        let n = self.meta(numbers, MetaField::PrimaryTerm)?;
        out.doc_value(n, meta.primary_term);
        let n = self.meta(numbers, MetaField::Source)?;
        let vector_fields: Vec<&str> = doc
            .fields
            .iter()
            .filter(|(_, v)| self.derived_source && matches!(v, FieldValue::Vector(_)))
            .map(|(name, _)| name.as_str())
            .collect();
        out.stored(
            n,
            Stored::Binary(if vector_fields.is_empty() {
                // ALLOC: the document's source, kept as `_source`; bounded by the HTTP body bound.
                meta.source.to_vec()
            } else {
                derived::mask_vectors(meta.source, &vector_fields)?
            }),
        );
        let n = self.meta(numbers, MetaField::Version)?;
        out.doc_value(n, meta.version);
        self.add_fields(numbers, &mut out, mapping, &doc.fields)?;
        if !doc.ignored.is_empty() {
            let n = self.meta(numbers, MetaField::Ignored)?;
            // `ParseContext`'s ignored fields are a `HashSet`, which
            // `IgnoredFieldMapper.postParse` iterates.
            // ALLOC: one name a malformed field, at most the document's fields.
            let names: Vec<&str> = doc.ignored.iter().map(String::as_str).collect();
            for i in hash_order(&names) {
                let Some(name) = doc.ignored.get(i) else {
                    continue;
                };
                out.term(n, name.as_bytes());
                // ALLOC: one name a malformed field, at most the document's fields.
                out.stored(n, Stored::String(name.clone()));
            }
        }
        Ok(out.finish())
    }

    /// Each value's fields, as its mapper's `parseCreateField` adds them,
    /// and its path's `_field_names` terms.
    fn add_fields(
        &mut self,
        numbers: &mut dyn FieldNumbers,
        out: &mut Builder,
        mapping: &Mapping,
        fields: &[(String, FieldValue)],
    ) -> Result<(), MappingError> {
        for (name, value) in fields {
            // The mapping, not the spec a field was first registered with:
            // an update may have changed it.
            let leaf = leaf(mapping.root(), name).ok_or_else(|| {
                // ALLOC: error path; one message.
                MappingError::illegal_state(&format!("no mapper for field [{name}]"))
            })?;
            let spec = Spec::of(leaf);
            if !spec.adds_anything() {
                continue;
            }
            let number = self.slot(numbers, name, Some(spec), None)?;
            let module = leaf
                .get("analyzer")
                .and_then(Value::text)
                .and_then(|a| self.modules.get(a))
                .map(Arc::as_ref);
            self.value(out, number, name, &spec, value, module)?;
            if spec.field_names() {
                let n = self.meta(numbers, MetaField::FieldNames)?;
                let mut end = 0;
                while let Some(dot) = name.get(end..).and_then(|rest| rest.find('.')) {
                    end = end.saturating_add(dot);
                    out.term(n, name.get(..end).unwrap_or(name).as_bytes());
                    end = end.saturating_add(1);
                }
                out.term(n, name.as_bytes());
            }
        }
        Ok(())
    }

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
        let (root, root_vectors) = self.document_with_vectors(numbers, mapping, doc, meta, routing)?;
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

    fn value(
        &self,
        out: &mut Builder,
        number: i32,
        name: &str,
        spec: &Spec,
        value: &FieldValue,
        module: Option<&dyn Analyze>,
    ) -> Result<(), MappingError> {
        match (spec.shape, value) {
            (Shape::Vector, FieldValue::Vector(components)) => out.vector(name, components),
            (Shape::Text { gap }, FieldValue::Text(text)) => {
                if spec.inverted() {
                    out.inverted(number, spec)
                        .analyzed(&self.analyzer, module, text, gap)?;
                }
                if spec.store {
                    // ALLOC: one stored value, bounded by the document's bytes.
                    out.stored(number, Stored::String(text.clone()));
                }
            }
            (Shape::Keyword, FieldValue::Keyword(term)) => {
                if spec.inverted() {
                    out.inverted(number, spec).keyword(term.as_bytes());
                }
                if spec.store {
                    // ALLOC: one stored value, bounded by the document's bytes.
                    out.stored(number, Stored::Binary(term.as_bytes().to_vec()));
                }
                if spec.doc_values {
                    // ALLOC: one doc value, bounded by the document's bytes.
                    out.doc_values.push(StoredField {
                        field_number: number,
                        value: Stored::Binary(term.as_bytes().to_vec()),
                    });
                }
            }
            (Shape::Bool, FieldValue::Bool(b)) => {
                let text = if *b { "T" } else { "F" };
                if spec.inverted() {
                    out.inverted(number, spec).keyword(text.as_bytes());
                }
                if spec.store {
                    // ALLOC: one stored flag, one byte.
                    out.stored(number, Stored::String(text.to_owned()));
                }
                if spec.doc_values {
                    out.doc_value(number, i64::from(*b));
                }
            }
            (Shape::Date, FieldValue::Date(millis)) => {
                out.number(
                    number,
                    spec,
                    long_point(*millis),
                    *millis,
                    Stored::Long(*millis),
                );
            }
            (Shape::Int, FieldValue::Number(Num::Int(i))) => {
                out.number(number, spec, int_point(*i), i64::from(*i), Stored::Int(*i));
            }
            (Shape::Long, FieldValue::Number(Num::Long(l))) => {
                out.number(number, spec, long_point(*l), *l, Stored::Long(*l));
            }
            (Shape::Float, FieldValue::Number(Num::Float(f))) => {
                let sortable = float_to_sortable_int(*f);
                out.number(
                    number,
                    spec,
                    int_point(sortable),
                    i64::from(sortable),
                    Stored::Float(*f),
                );
            }
            (Shape::Double, FieldValue::Number(Num::Double(d))) => {
                let sortable = double_to_sortable_long(*d);
                out.number(
                    number,
                    spec,
                    long_point(sortable),
                    sortable,
                    Stored::Double(*d),
                );
            }
            (shape, value) => {
                // ALLOC: error path; one message.
                return Err(MappingError::illegal_state(&format!(
                    "a {value:?} for a field indexed as {shape:?}"
                )));
            }
        }
        Ok(())
    }
}
