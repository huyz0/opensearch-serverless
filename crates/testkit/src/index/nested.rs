//! The nested objects of a [`Spec::with_nested`] index.

use lucene_codecs::stored_fields::{FieldValue, StoredField};
use lucene_index::index_writer::{ExplicitDocument, ExplicitFields, InvertedField};

use lucene_codecs::field_infos::{DocValuesSkipIndexType, DocValuesType, FieldInfo, IndexOptions};
use lucene_index::index_writer::IndexWriter;

use super::{Spec, document, term, vector_number};

/// How many nested children each document has in a [`Spec::with_nested`] index.
pub const NESTED_CHILDREN: usize = 2;

/// The `c.w` of child `j` of document `i`: `w<i>-<j>`.
pub fn nested_word(i: usize, j: usize) -> String {
    format!("w{i}-{j}")
}

/// The engine numbers fields in registration order, and the vector field
/// takes a number only when there is one.
fn nested_path_number(spec: &Spec) -> i32 {
    vector_number(spec) + i32::from(spec.vectors.is_some())
}

fn primary_term_number(spec: &Spec) -> i32 {
    nested_path_number(spec) + 1
}

fn nested_word_number(spec: &Spec) -> i32 {
    nested_path_number(spec) + 2
}

/// The block of document `i` of a nested index: its children, then it.
fn block(spec: &Spec, i: usize) -> Vec<ExplicitDocument> {
    let mut docs: Vec<ExplicitDocument> = (0..NESTED_CHILDREN)
        .map(|j| {
            let word = nested_word(i, j);
            ExplicitDocument {
                stored: Vec::new(),
                fields: ExplicitFields {
                    inverted: vec![
                        InvertedField {
                            field_number: nested_path_number(spec),
                            terms: vec![term(b"c")].into(),
                            norm: None,
                        },
                        InvertedField {
                            field_number: nested_word_number(spec),
                            terms: vec![term(word.as_bytes())].into(),
                            norm: None,
                        },
                    ],
                    doc_values: vec![StoredField {
                        field_number: nested_word_number(spec),
                        value: FieldValue::Binary(word.into_bytes()),
                    }],
                    ..ExplicitFields::default()
                },
            }
        })
        .collect();
    let mut parent = document(spec, i);
    parent.fields.doc_values.push(StoredField {
        field_number: primary_term_number(spec),
        value: FieldValue::Long(1),
    });
    docs.push(parent);
    docs
}

/// Registers the nested index's three extra fields, in the order the
/// numbers above count them.
pub(super) fn register(w: &mut IndexWriter<'_>, spec: &Spec) {
    if !spec.nested {
        return;
    }
    let docs_only = |name: &str, number| {
        FieldInfo::new(name, number)
            .with_index_options(IndexOptions::Docs)
            .with_omit_norms(true)
    };
    w.register_field(docs_only("_nested_path", nested_path_number(spec)))
        .expect("_nested_path");
    w.register_field(
        FieldInfo::new("_primary_term", primary_term_number(spec)).with_doc_values(
            DocValuesType::Numeric,
            DocValuesSkipIndexType::None,
            -1,
        ),
    )
    .expect("_primary_term");
    w.register_field(docs_only("c.w", nested_word_number(spec)).with_doc_values(
        DocValuesType::SortedSet,
        DocValuesSkipIndexType::None,
        -1,
    ))
    .expect("c.w");
}

/// Adds every document of a nested index as a block, committing after each
/// segment's worth; the number added, 0 for an index that is not nested.
pub(super) fn add_blocks(w: &mut IndexWriter<'_>, spec: &Spec) -> usize {
    let mut start = 0;
    if !spec.nested {
        return start;
    }
    while start < spec.docs {
        let end = spec.docs.min(start + spec.docs_per_segment);
        for i in start..end {
            let docs = block(spec, i);
            let vectors = docs.iter().map(|_| Vec::new()).collect();
            w.add_explicit_documents_with_vectors(None, docs, vectors)
                .expect("add");
        }
        w.commit().expect("commit");
        start = end;
    }
    start
}

/// The nested object's mapping, after the other fields'.
pub(super) fn mapping(spec: &Spec, out: &mut String) {
    if spec.nested {
        out.push_str(r#","c":{"type":"nested","properties":{"w":{"type":"keyword"}}}"#);
    }
}

/// The nested objects of document `i`'s `_source`, after the other fields'.
pub(super) fn source(spec: &Spec, i: usize, out: &mut String) {
    if spec.nested {
        let children: Vec<String> = (0..NESTED_CHILDREN)
            .map(|j| format!(r#"{{"w":"{}"}}"#, nested_word(i, j)))
            .collect();
        out.push_str(&format!(r#","c":[{}]"#, children.join(",")));
    }
}

impl Spec {
    /// Gives every document [`NESTED_CHILDREN`] nested children `c`.
    pub fn with_nested(mut self) -> Self {
        self.nested = true;
        self
    }
}
