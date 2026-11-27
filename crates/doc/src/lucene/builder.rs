//! One Lucene document as it is built: stored values in order, each
//! field's postings, doc values and points.

use std::collections::HashMap;

use lucene_codecs::field_infos::IndexOptions;
use lucene_codecs::stored_fields::{FieldValue as Stored, StoredField};
use lucene_index::index_writer::{DocumentVector, ExplicitDocument, ExplicitFields};

use lucene_util::half_float::half_float_to_sortable_short;

use super::encode::{
    double_to_sortable_long, float_to_sortable_int, int_point, long_point, short_point,
};
use super::fields::{Shape, Spec};
use super::invert::Inverted;

/// A Lucene document under construction.
#[derive(Default)]
pub(super) struct Builder {
    stored: Vec<StoredField>,
    /// Fields with postings, in the order first added.
    inverted: Vec<Inverted>,
    /// Where each field number's postings sit in `inverted`.
    inverted_at: HashMap<i32, usize>,
    pub(super) doc_values: Vec<StoredField>,
    points: Vec<StoredField>,
    vectors: Vec<DocumentVector>,
}

impl Builder {
    pub(super) fn stored(&mut self, number: i32, value: Stored) {
        // ALLOC: one stored value, bounded by the document's bytes.
        self.stored.push(StoredField {
            field_number: number,
            value,
        });
    }

    pub(super) fn vector(&mut self, name: &str, components: &[f32]) {
        // ALLOC: one vector, at most one a vector field of the document; bounded by the dimension.
        self.vectors
            .push(DocumentVector::float32(name, components.to_vec()));
    }

    pub(super) fn doc_value(&mut self, number: i32, value: i64) {
        // ALLOC: one doc value, at most one a value of the document.
        self.doc_values.push(StoredField {
            field_number: number,
            value: Stored::Long(value),
        });
    }

    pub(super) fn point(&mut self, number: i32, packed: Vec<u8>) {
        // ALLOC: one point, at most one a value of the document.
        self.points.push(StoredField {
            field_number: number,
            value: Stored::Binary(packed),
        });
    }

    pub(super) fn number(
        &mut self,
        number: i32,
        spec: &Spec,
        point: Vec<u8>,
        doc_value: i64,
        stored: Stored,
    ) {
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

    /// A `float`: its sortable int, as the point and the doc value.
    pub(super) fn float(&mut self, number: i32, spec: &Spec, f: f32) {
        let s = float_to_sortable_int(f);
        self.number(number, spec, int_point(s), i64::from(s), Stored::Float(f));
    }

    /// A `double`: its sortable long, as the point and the doc value.
    pub(super) fn double(&mut self, number: i32, spec: &Spec, d: f64) {
        let s = double_to_sortable_long(d);
        self.number(number, spec, long_point(s), s, Stored::Double(d));
    }

    /// A `half_float`: its sortable short in a 2-byte point and as the doc value.
    pub(super) fn half_float(&mut self, number: i32, spec: &Spec, f: f32) {
        let s = half_float_to_sortable_short(f);
        self.number(number, spec, short_point(s), i64::from(s), Stored::Float(f));
    }

    /// The postings of field `number`, created on its first value.
    pub(super) fn inverted(&mut self, number: i32, spec: &Spec) -> &mut Inverted {
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
    pub(super) fn term(&mut self, number: i32, term: &[u8]) {
        let spec = Spec {
            shape: Shape::Keyword,
            index_options: IndexOptions::Docs,
            points: false,
            norms: false,
            doc_values: false,
            store: false,
            vector: None,
        };
        self.inverted(number, &spec).keyword(term);
    }

    pub(super) fn finish(self) -> (ExplicitDocument, Vec<DocumentVector>) {
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
        let document = ExplicitDocument {
            stored: self.stored,
            fields: ExplicitFields {
                inverted,
                doc_values: self.doc_values,
                points: self.points,
                registered,
                ..ExplicitFields::default()
            },
        };
        (document, self.vectors)
    }
}
