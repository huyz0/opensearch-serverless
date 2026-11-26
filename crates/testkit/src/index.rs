//! A one-shard index written with the engine and laid out as the blobs the
//! Java shell publishes, for tests that need more documents than a recorded
//! fixture holds.
//!
//! The layout is the shell's (fixtures/compat-blobs/README.md): the
//! descriptor register at `indices/<name>`, the commit register at
//! `segments/<name>#<uuid>#0/manifest`, and each Lucene file whole at
//! `segments/<name>#<uuid>#0/t=1/<file>`, which the manifest records as the
//! location `t=1`. Registers are an 8-byte generation, then the value.
//!
//! Document `i` has the fields OpenSearch writes for the mapping
//! [`mapping`]: `_id` (stored and indexed, encoded as `Uid.encodeId` encodes
//! a non-numeric, non-base64 id), `_source` (stored), `tag` (a keyword:
//! indexed and sorted-set doc values) and `n` (a long: a point and
//! sorted-numeric doc values), and as many more keywords `k0`, `k1`, ... as
//! [`Spec::keywords`] asks for, and text fields `x0`, `x1`, ... (indexed
//! with positions and norms) as [`Spec::texts`] asks for.

// ARITH: document numbers and field counts are a test's small constants;
// an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_codecs::field_infos::{
    DocValuesSkipIndexType, DocValuesType, FieldInfo, IndexOptions, VectorEncoding,
    VectorSimilarityFunction,
};
use lucene_codecs::stored_fields::{FieldValue, StoredField};
use lucene_index::index_writer::{
    DocumentVector, ExplicitDocument, ExplicitFields, IndexWriter, InvertedField, InvertedTerm,
};
use lucene_index::segment_info::LuceneVersion;
use lucene_store::{ByteBuffersDirectory, Directory};

mod nested;
pub use nested::{NESTED_CHILDREN, nested_deep_word, nested_word};

/// The mapping of `spec`'s index.
pub fn mapping(spec: &Spec) -> String {
    let mut out = String::from(r#"{"properties":{"tag":{"type":"keyword"},"n":{"type":"long"}"#);
    for k in 0..spec.keywords {
        out.push_str(&format!(r#","k{k}":{{"type":"keyword"}}"#));
    }
    for x in 0..spec.texts {
        out.push_str(&format!(r#","x{x}":{{"type":"text"}}"#));
    }
    if let Some(similarity) = spec.vectors {
        let space = match similarity {
            VectorSimilarityFunction::Cosine => "cosinesimil",
            VectorSimilarityFunction::MaximumInnerProduct => "innerproduct",
            _ => "l2",
        };
        let dim = spec.vector_dimension;
        out.push_str(&format!(
            r#","v":{{"type":"knn_vector","dimension":{dim},"method":{{"name":"hnsw","engine":"lucene","space_type":"{space}"}}}}"#
        ));
    }
    nested::mapping(spec, &mut out);
    out.push_str("}}");
    out
}

/// The vector of document `i` when [`Spec::with_vectors`] is set: three small
/// whole numbers, and none for every fifth document.
pub fn doc_vector(i: usize) -> Option<[f32; 3]> {
    (i % 5 != 4).then_some([(i % 7) as f32, (i % 5) as f32, (i % 3) as f32])
}

/// The vector of document `i` of `spec`: [`doc_vector`] for three
/// dimensions, otherwise `vector_dimension` pseudo-random components drawn
/// from `i`, and none for every fifth document.
pub fn spec_vector(spec: &Spec, i: usize) -> Option<Vec<f32>> {
    if spec.vector_dimension == 3 {
        return doc_vector(i).map(|v| v.to_vec());
    }
    (i % 5 != 4).then(|| {
        let mut state = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..spec.vector_dimension)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 40) as f32 / (1u64 << 24) as f32
            })
            .collect()
    })
}

/// How many distinct `tag` values the documents cycle through.
pub const TAGS: usize = 16;

const ID: i32 = 0;
const SOURCE: i32 = 1;
const TAG: i32 = 2;
const N: i32 = 3;
const FIRST_KEYWORD: i32 = 4;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// What to build.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The index name; its uuid is `<name>-uuid`.
    pub name: String,
    /// How many documents.
    pub docs: usize,
    /// A commit after every this many documents, each flushing a segment.
    pub docs_per_segment: usize,
    /// Bytes of padding in each document's `_source`.
    pub source_padding: usize,
    /// How many keyword fields beyond `tag`.
    pub keywords: usize,
    /// How many text fields.
    pub texts: usize,
    /// Whether flushed segments are compound files, as the shell writes
    /// them (Lucene's default).
    pub compound: bool,
    /// The similarity of the `v` vector field ([`doc_vector`]), when there is one.
    pub vectors: Option<VectorSimilarityFunction>,
    /// The dimension of the `v` vector field: 3 ([`doc_vector`]) unless set.
    pub vector_dimension: usize,
    /// The index's settings, as the JSON object its descriptor holds.
    pub settings: String,
    /// The depth of nested objects: 1 for children `c` ([`Spec::with_nested`]), 2
    /// for grandchildren `c.d` too ([`Spec::with_nested_deep`]).
    pub nested: u8,
}

impl Spec {
    /// `docs` documents in one segment, with no padding.
    pub fn new(name: &str, docs: usize) -> Self {
        Self {
            name: name.to_owned(),
            docs,
            docs_per_segment: docs.max(1),
            source_padding: 0,
            compound: true,
            keywords: 0,
            texts: 0,
            vectors: None,
            vector_dimension: 3,
            settings: "{}".to_owned(),
            nested: 0,
        }
    }

    /// Commits after every `docs` documents.
    pub fn segments_of(mut self, docs: usize) -> Self {
        self.docs_per_segment = docs.max(1);
        self
    }

    /// Adds `n` keyword fields, `k0` to `k<n-1>`.
    pub fn with_keywords(mut self, n: usize) -> Self {
        self.keywords = n;
        self
    }

    /// Adds `n` text fields, `x0` to `x<n-1>`.
    pub fn with_texts(mut self, n: usize) -> Self {
        self.texts = n;
        self
    }

    /// Adds the 3-dimensional vector field `v`, one document in five without a value.
    pub fn with_vectors(mut self, similarity: VectorSimilarityFunction) -> Self {
        self.vectors = Some(similarity);
        self
    }

    /// Gives the vector field `dimension` components, drawn from a fixed
    /// generator ([`spec_vector`]), for a test that needs a realistic size.
    pub fn with_vector_dimension(mut self, dimension: usize) -> Self {
        self.vector_dimension = dimension;
        self
    }

    /// Gives the index these settings, a JSON object of flat dotted keys.
    pub fn with_settings(mut self, json: &str) -> Self {
        json.clone_into(&mut self.settings);
        self
    }

    /// Writes each segment as separate files rather than a compound file.
    pub fn separate_files(mut self) -> Self {
        self.compound = false;
        self
    }

    /// Pads each `_source` with `bytes` bytes.
    pub fn padded(mut self, bytes: usize) -> Self {
        self.source_padding = bytes;
        self
    }
}

/// Document `i`'s id.
pub fn doc_id(i: usize) -> String {
    format!("doc.{i}")
}

/// Document `i`'s `tag`.
pub fn doc_tag(i: usize) -> String {
    format!("t{}", i % TAGS)
}

/// Document `i`'s value of keyword `k<k>`.
pub fn doc_keyword(i: usize, k: usize) -> String {
    format!("v{}", (i + k) % 7)
}

/// Document `i`'s `_source` in `spec`'s index.
pub fn doc_source(spec: &Spec, i: usize) -> String {
    let mut out = format!(r#"{{"tag":"{}","n":{i}"#, doc_tag(i));
    for k in 0..spec.keywords {
        out.push_str(&format!(r#","k{k}":"{}""#, doc_keyword(i, k)));
    }
    for x in 0..spec.texts {
        out.push_str(&format!(r#","x{x}":"{}""#, doc_words(i, x).join(" ")));
    }
    nested::source(spec, i, &mut out);
    out.push_str(&format!(
        r#","pad":"{}"}}"#,
        "x".repeat(spec.source_padding)
    ));
    out
}

/// Document `i`'s two words of text field `x<x>`.
pub fn doc_words(i: usize, x: usize) -> [String; 2] {
    [
        format!("w{}", (i + x) % 50),
        format!("u{}", (i * 7 + x) % 13),
    ]
}

fn keyword_number(k: usize) -> i32 {
    FIRST_KEYWORD + i32::try_from(k).expect("fits")
}

fn text_number(spec: &Spec, x: usize) -> i32 {
    keyword_number(spec.keywords) + i32::try_from(x).expect("fits")
}

fn vector_number(spec: &Spec) -> i32 {
    text_number(spec, spec.texts)
}

fn term(bytes: &[u8]) -> InvertedTerm {
    InvertedTerm {
        term: bytes.to_vec(),
        freq: 1,
        positions: Vec::new(),
        offsets: Vec::new(),
        payloads: Vec::new(),
    }
}

/// `Uid.encodeId` of an id that is neither numeric nor URL-safe base64.
fn encode_id(id: &str) -> Vec<u8> {
    let mut out = vec![0xff];
    out.extend_from_slice(id.as_bytes());
    out
}

/// `NumericUtils.longToSortableBytes`.
fn sortable(v: i64) -> Vec<u8> {
    (v ^ i64::MIN).to_be_bytes().to_vec()
}

fn document(spec: &Spec, i: usize) -> ExplicitDocument {
    let id = encode_id(&doc_id(i));
    let tag = doc_tag(i);
    let n = i64::try_from(i).expect("fits");
    let mut doc = ExplicitDocument {
        stored: vec![
            StoredField {
                field_number: ID,
                value: FieldValue::Binary(id.clone()),
            },
            StoredField {
                field_number: SOURCE,
                value: FieldValue::Binary(doc_source(spec, i).into_bytes()),
            },
        ],
        fields: ExplicitFields {
            inverted: vec![
                InvertedField {
                    field_number: ID,
                    terms: vec![term(&id)].into(),
                    norm: None,
                },
                InvertedField {
                    field_number: TAG,
                    terms: vec![term(tag.as_bytes())].into(),
                    norm: None,
                },
            ],
            doc_values: vec![
                StoredField {
                    field_number: TAG,
                    value: FieldValue::Binary(tag.into_bytes()),
                },
                StoredField {
                    field_number: N,
                    value: FieldValue::Long(n),
                },
            ],
            points: vec![StoredField {
                field_number: N,
                value: FieldValue::Binary(sortable(n)),
            }],
            ..ExplicitFields::default()
        },
    };
    for k in 0..spec.keywords {
        let value = doc_keyword(i, k);
        doc.fields.inverted.push(InvertedField {
            field_number: keyword_number(k),
            terms: vec![term(value.as_bytes())].into(),
            norm: None,
        });
        doc.fields.doc_values.push(StoredField {
            field_number: keyword_number(k),
            value: FieldValue::Binary(value.into_bytes()),
        });
    }
    for x in 0..spec.texts {
        let words = doc_words(i, x);
        let at = |w: &String, p: i32| InvertedTerm {
            positions: vec![p],
            ..term(w.as_bytes())
        };
        doc.fields.inverted.push(InvertedField {
            field_number: text_number(spec, x),
            terms: vec![at(&words[0], 0), at(&words[1], 1)].into(),
            // BM25's norm of two terms: `SmallFloat.intToByte4(2)`.
            norm: Some(2),
        });
    }
    doc
}

fn register(value: &str) -> Vec<u8> {
    let mut out = 1u64.to_be_bytes().to_vec();
    out.extend_from_slice(value.as_bytes());
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The files of `spec`'s index as the engine wrote them, by name.
///
/// # Panics
///
/// If the engine refuses to write: a test cannot run without its index.
pub fn lucene_files(spec: &Spec) -> Vec<(String, Vec<u8>)> {
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("open writer");
        w.enable_explicit_documents().expect("explicit documents");
        w.set_use_compound_file(spec.compound);
        let docs_only = |name: &str, number| {
            FieldInfo::new(name, number)
                .with_index_options(IndexOptions::Docs)
                .with_omit_norms(true)
        };
        w.register_field(docs_only("_id", ID)).expect("_id");
        w.register_field(FieldInfo::new("_source", SOURCE))
            .expect("_source");
        w.register_field(docs_only("tag", TAG).with_doc_values(
            DocValuesType::SortedSet,
            DocValuesSkipIndexType::None,
            -1,
        ))
        .expect("tag");
        w.register_field(FieldInfo::new("n", N).with_points(1, 1, 8).with_doc_values(
            DocValuesType::SortedNumeric,
            DocValuesSkipIndexType::None,
            -1,
        ))
        .expect("n");
        for k in 0..spec.keywords {
            w.register_field(
                docs_only(&format!("k{k}"), keyword_number(k)).with_doc_values(
                    DocValuesType::SortedSet,
                    DocValuesSkipIndexType::None,
                    -1,
                ),
            )
            .expect("keyword");
        }
        for x in 0..spec.texts {
            w.register_field(
                FieldInfo::new(format!("x{x}"), text_number(spec, x))
                    .with_index_options(IndexOptions::DocsAndFreqsAndPositions),
            )
            .expect("text");
        }
        if let Some(similarity) = spec.vectors {
            w.register_field(FieldInfo::new("v", vector_number(spec)).with_vectors(
                i32::try_from(spec.vector_dimension).expect("dimension"),
                VectorEncoding::Float32,
                similarity,
            ))
            .expect("v");
        }
        nested::register(&mut w, spec);
        let mut start = nested::add_blocks(&mut w, spec);
        while start < spec.docs {
            let end = spec.docs.min(start + spec.docs_per_segment);
            let docs: Vec<ExplicitDocument> = (start..end).map(|i| document(spec, i)).collect();
            let vectors = (start..end)
                .map(|i| {
                    spec_vector(spec, i)
                        .filter(|_| spec.vectors.is_some())
                        .map(|v| DocumentVector::float32("v", v))
                        .into_iter()
                        .collect()
                })
                .collect();
            w.add_explicit_documents_with_vectors(None, docs, vectors)
                .expect("add");
            w.commit().expect("commit");
            start = end;
        }
        if spec.docs == 0 {
            w.commit().expect("commit");
        }
    }
    dir.list_all()
        .expect("list")
        .into_iter()
        .filter(|name| name != "write.lock")
        .map(|name| {
            let input = dir.open(&name).expect("open file");
            let bytes = input.to_vec();
            (name, bytes)
        })
        .collect()
}

/// Every blob of `spec`'s index under `base` (empty, or ending in `/`), by
/// key: the descriptor, the commit manifest, and one blob a Lucene file.
///
/// # Panics
///
/// As [`lucene_files`].
pub fn blobs(base: &str, spec: &Spec) -> Vec<(String, Vec<u8>)> {
    let name = &spec.name;
    let uuid = format!("{name}-uuid");
    let shard = format!("{base}segments/{name}#{uuid}#0/");
    let files = lucene_files(spec);
    let mut out = Vec::with_capacity(files.len() + 2);
    out.push((
        format!("{base}indices/{name}"),
        register(&format!(
            r#"{{"name":"{name}","uuid":"{uuid}","number_of_shards":1,"mapping":{},"settings":{}}}"#,
            json_string(&mapping(spec)),
            spec.settings
        )),
    ));
    let locations: Vec<String> = files
        .iter()
        .map(|(f, _)| format!(r#""{f}":"t=1""#))
        .collect();
    let lengths: Vec<String> = files
        .iter()
        .map(|(f, b)| format!(r#""{f}":{}"#, b.len()))
        .collect();
    out.push((
        format!("{shard}manifest"),
        register(&format!(
            r#"{{"term":1,"writer":"testkit","files":{{{}}},"lengths":{{{}}},"wal_ordinal":0,"stage_seq":1}}"#,
            locations.join(","),
            lengths.join(",")
        )),
    ));
    for (file, bytes) in files {
        out.push((format!("{shard}t=1/{file}"), bytes));
    }
    out
}

#[cfg(test)]
mod tests;
