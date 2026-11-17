//! A parsed document as the Lucene document OpenSearch indexes for it: the
//! metadata fields, then each value's fields as its mapper's
//! `parseCreateField` adds them, then `_ignored`; and the numbers a
//! segment's fields get, in the order an `IndexingChain` first sees them.
//!
//! Ported from the field mappers' `parseCreateField`, `IdFieldMapper`,
//! `SeqNoFieldMapper`, `VersionFieldMapper`, `SourceFieldMapper`,
//! `FieldNamesFieldMapper` and `IgnoredFieldMapper` (server, OpenSearch
//! 3.10), and `IndexingChain.invert`'s positions, offsets and lengths.

pub mod encode;
pub mod fields;
mod invert;

use std::collections::HashMap;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::FieldInfo;
use lucene_codecs::stored_fields::{FieldValue as Stored, StoredField};
use lucene_index::index_writer::{ExplicitDocument, ExplicitFields};

use crate::document::{FieldValue, ParsedDocument};
use crate::mapping::{Leaf, Mapper, Mapping, MappingError, Num, Object};

use encode::{double_to_sortable_long, encode_id, float_to_sortable_int, int_point, long_point};
use fields::{Meta as MetaField, Shape, Spec};
use invert::Inverted;

/// A document's metadata, given by the index request and the engine.
#[derive(Clone, Copy, Debug)]
pub struct Meta<'a> {
    /// The document's id.
    pub id: &'a str,
    /// Its sequence number.
    pub seq_no: i64,
    /// The primary term it was indexed under.
    pub primary_term: i64,
    /// Its version.
    pub version: i64,
    /// Its source, stored as given.
    pub source: &'a [u8],
}

/// A field a segment knows: its number and, for a mapped field, its spec.
#[derive(Clone, Copy, Debug)]
struct Slot {
    number: i32,
    spec: Option<Spec>,
}

/// Builds a segment's Lucene documents: the analyzer, and the fields the
/// segment has numbered so far.
pub struct Indexer {
    analyzer: Analyzer,
    slots: HashMap<String, Slot>,
    next: i32,
    /// Fields numbered since [`Indexer::new_fields`] last took them.
    fresh: Vec<FieldInfo>,
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}

impl Indexer {
    /// An indexer for a new segment, with no field numbered yet.
    pub fn new() -> Self {
        Indexer {
            // The `standard` analyzer as OpenSearch builds it: no stop words.
            analyzer: Analyzer::standard(None),
            // ALLOC: one entry a field of the segment, at most the mapping's total fields limit plus the metadata fields.
            slots: HashMap::new(),
            next: 0,
            // ALLOC: the fields the last document added, at most the total fields limit.
            fresh: Vec::new(),
        }
    }

    /// The fields numbered since the last call, in number order, to
    /// register with the engine before the document that added them.
    pub fn new_fields(&mut self) -> Vec<FieldInfo> {
        std::mem::take(&mut self.fresh)
    }

    fn slot(&mut self, name: &str, spec: Option<Spec>, meta: Option<MetaField>) -> Slot {
        if let Some(s) = self.slots.get(name) {
            return *s;
        }
        let number = self.next;
        self.next = self.next.saturating_add(1);
        let info = match (spec, meta) {
            (Some(s), _) => s.field_info(name, number),
            (None, Some(m)) => m.field_info(number),
            (None, None) => FieldInfo::new(name, number),
        };
        // ALLOC: one field info a new field, bounded as `fresh`.
        self.fresh.push(info);
        let slot = Slot { number, spec };
        // ALLOC: one entry a new field, bounded as `slots`.
        self.slots.insert(name.to_owned(), slot);
        slot
    }

    fn meta(&mut self, field: MetaField) -> i32 {
        self.slot(field.name(), None, Some(field)).number
    }

    /// The Lucene document for `doc`, read against `mapping` (the mapping
    /// grown by `doc`, when it grew one).
    ///
    /// # Errors
    ///
    /// An empty id, or a field the mapping does not map.
    pub fn document(
        &mut self,
        mapping: &Mapping,
        doc: &ParsedDocument,
        meta: &Meta<'_>,
    ) -> Result<ExplicitDocument, MappingError> {
        // ALLOC: one document's Lucene fields, bounded by the document's bytes (ADR 0017's reservation).
        let mut out = Builder::default();
        let id = encode_id(meta.id)?;
        let n = self.meta(MetaField::Id);
        out.term(n, &id);
        out.stored(n, Stored::Binary(id));
        let n = self.meta(MetaField::SeqNo);
        out.point(n, long_point(meta.seq_no));
        out.doc_value(n, meta.seq_no);
        let n = self.meta(MetaField::PrimaryTerm);
        out.doc_value(n, meta.primary_term);
        let n = self.meta(MetaField::Source);
        // ALLOC: the document's source, kept as `_source`; bounded by the HTTP body bound.
        out.stored(n, Stored::Binary(meta.source.to_vec()));
        let n = self.meta(MetaField::Version);
        out.doc_value(n, meta.version);
        for (name, value) in &doc.fields {
            let spec = match self.slots.get(name.as_str()) {
                Some(Slot { spec: Some(s), .. }) => *s,
                _ => {
                    // ALLOC: error path; one message.
                    let leaf = leaf(mapping.root(), name).ok_or_else(|| {
                        MappingError::illegal_state(&format!("no mapper for field [{name}]"))
                    })?;
                    Spec::of(leaf)
                }
            };
            if !spec.adds_anything() {
                continue;
            }
            let number = self.slot(name, Some(spec), None).number;
            self.value(&mut out, number, &spec, value)?;
            if spec.field_names() {
                let n = self.meta(MetaField::FieldNames);
                let mut end = 0;
                while let Some(dot) = name.get(end..).and_then(|rest| rest.find('.')) {
                    end = end.saturating_add(dot);
                    out.term(n, name.get(..end).unwrap_or(name).as_bytes());
                    end = end.saturating_add(1);
                }
                out.term(n, name.as_bytes());
            }
        }
        if !doc.ignored.is_empty() {
            let n = self.meta(MetaField::Ignored);
            for name in &doc.ignored {
                out.term(n, name.as_bytes());
                // ALLOC: one name a malformed field, at most the document's fields.
                out.stored(n, Stored::String(name.clone()));
            }
        }
        Ok(out.finish())
    }

    fn value(
        &self,
        out: &mut Builder,
        number: i32,
        spec: &Spec,
        value: &FieldValue,
    ) -> Result<(), MappingError> {
        match (spec.shape, value) {
            (Shape::Text { gap }, FieldValue::Text(text)) => {
                if spec.inverted() {
                    out.inverted(number, spec).text(&self.analyzer, text, gap)?;
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

/// The leaf at `full`, a full field name, multi-fields included.
fn leaf<'m>(root: &'m Object, full: &str) -> Option<&'m Leaf> {
    let mut obj = root;
    let mut rest = full;
    loop {
        let (head, tail) = rest
            .split_once('.')
            .map_or((rest, None), |(h, t)| (h, Some(t)));
        match (obj.child(head)?, tail) {
            (Mapper::Object(o), Some(t)) => {
                obj = o;
                rest = t;
            }
            (Mapper::Leaf(l), None) => return Some(l),
            (Mapper::Leaf(l), Some(t)) => {
                return l
                    .multi_fields()
                    .iter()
                    .find(|(name, _)| name == t)
                    .map(|(_, m)| m);
            }
            (Mapper::Object(_), None) => return None,
        }
    }
}

/// One Lucene document as it is built: stored values in order, each
/// field's postings, doc values and points.
#[derive(Default)]
struct Builder {
    stored: Vec<StoredField>,
    /// Fields with postings, in the order first added.
    inverted: Vec<Inverted>,
    /// Where each field number's postings sit in `inverted`.
    inverted_at: HashMap<i32, usize>,
    doc_values: Vec<StoredField>,
    points: Vec<StoredField>,
}

impl Builder {
    fn stored(&mut self, number: i32, value: Stored) {
        // ALLOC: one stored value, bounded by the document's bytes.
        self.stored.push(StoredField {
            field_number: number,
            value,
        });
    }

    fn doc_value(&mut self, number: i32, value: i64) {
        // ALLOC: one doc value, at most one a value of the document.
        self.doc_values.push(StoredField {
            field_number: number,
            value: Stored::Long(value),
        });
    }

    fn point(&mut self, number: i32, packed: Vec<u8>) {
        // ALLOC: one point, at most one a value of the document.
        self.points.push(StoredField {
            field_number: number,
            value: Stored::Binary(packed),
        });
    }

    fn number(&mut self, number: i32, spec: &Spec, point: Vec<u8>, doc_value: i64, stored: Stored) {
        if spec.points {
            self.point(number, point);
        }
        if spec.doc_values {
            self.doc_value(number, doc_value);
        }
        if spec.store {
            self.stored(number, stored);
        }
    }

    /// The postings of field `number`, created on its first value.
    fn inverted(&mut self, number: i32, spec: &Spec) -> &mut Inverted {
        let len = self.inverted.len();
        // ALLOC: one entry an indexed field of the document.
        let i = *self.inverted_at.entry(number).or_insert(len);
        if i == len {
            // ALLOC: as above.
            self.inverted
                .push(Inverted::new(number, spec.index_options, spec.norms));
        }
        &mut self.inverted[i]
    }

    /// A metadata field's one untokenized term.
    fn term(&mut self, number: i32, term: &[u8]) {
        let spec = Spec {
            shape: Shape::Keyword,
            index_options: lucene_codecs::field_infos::IndexOptions::Docs,
            points: false,
            norms: false,
            doc_values: false,
            store: false,
        };
        self.inverted(number, &spec).keyword(term);
    }

    fn finish(self) -> ExplicitDocument {
        // ALLOC: one entry an indexed field of the document.
        let mut inverted = Vec::with_capacity(self.inverted.len());
        // ALLOC: one entry a field whose values gave no token.
        let mut registered = Vec::new();
        for field in self.inverted {
            match field.finish() {
                Ok(f) => inverted.push(f),
                Err(number) => registered.push(number),
            }
        }
        ExplicitDocument {
            stored: self.stored,
            fields: ExplicitFields {
                inverted,
                doc_values: self.doc_values,
                points: self.points,
                registered,
                ..ExplicitFields::default()
            },
        }
    }
}
