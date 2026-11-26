//! Each fixture of `fixtures/segments/` holds a mapping, documents,
//! and the segment stock OpenSearch wrote for them, as `SegmentDump.java`
//! reads it. The writer parses the same documents, builds their Lucene
//! documents, writes them with the engine, and must dump the same: field
//! infos as the engine wrote them, and each document's stored fields, terms,
//! norms, doc values and points as handed to the engine.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
// A fixture is a small file the test reads whole.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod segment_dump;

use segment_dump::{document_json, field_json, read_back};
use std::collections::BTreeMap;

use std::path::{Path, PathBuf};

use lucene_codecs::field_infos::FieldInfo;
use lucene_codecs::stored_fields::{FieldValue, StoredField};
use lucene_index::index_writer::{ExplicitDocument, IndexWriter, VectorValue};
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_writer::document;
use oss_writer::index::{Expect, Shard};
use oss_writer::lucene::{Indexer, Meta};
use oss_writer::mapping::{Mapping, ParseOptions};
use serde_json::{Value, json};

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/segments");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    out.sort();
    out
}

/// The mapping a fixture creates its index with: a `knn_vector` is read
/// when its settings turn `index.knn` on.
fn fixture_mapping(fixture: &Value) -> Mapping {
    let knn = fixture["settings"]["index.knn"].as_bool().unwrap_or(false);
    match fixture.get("mappings") {
        Some(m) if knn => {
            Mapping::parse_with(m.to_string().as_bytes(), ParseOptions { knn: Some(true), ..ParseOptions::default() })
                .expect("mapping")
        }
        Some(m) => Mapping::parse(m.to_string().as_bytes()).expect("mapping"),
        None => Mapping::default(),
    }
}

fn recorded_source(fixture: &Value, doc: usize) -> String {
    let stored = fixture["segment"]["docs"][doc]["stored"]
        .as_array()
        .expect("stored");
    let source = stored.iter().find(|f| f[0] == "_source").expect("_source");
    source[2].as_str().expect("text").to_owned()
}

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn replay(path: &Path) {
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(path).expect("read")).expect("fixture json");
    let mut mapping = fixture_mapping(&fixture);
    let mut indexer = Indexer::new();
    let dir = ByteBuffersDirectory::new();
    let mut docs = Vec::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let documents = fixture["documents"].as_array().expect("documents");
        for (seq_no, d) in documents.iter().enumerate() {
            let id = d["id"].as_str().expect("id");
            // The bytes OpenSearch received, as its `_source` kept them:
            // `serde_json` would sort the keys.
            let source = recorded_source(&fixture, seq_no);
            let parsed = document::parse(&mapping, id, source.as_bytes())
                .unwrap_or_else(|e| panic!("{}: {id}: {e:?}", path.display()));
            if let Some(grown) = &parsed.grown {
                mapping = grown.clone();
            }
            let meta = Meta {
                id,
                seq_no: i64::try_from(seq_no).expect("fits"),
                primary_term: 1,
                version: 1,
                source: source.as_bytes(),
            };
            let (doc, vectors) = indexer
                .document_with_vectors(&mut w, &mapping, &parsed, &meta, None)
                .unwrap_or_else(|e| panic!("{}: {id}: {e:?}", path.display()));
            docs.push((doc.clone(), vectors.clone()));
            if vectors.is_empty() {
                w.add_explicit_documents(vec![doc]).expect("add");
            } else {
                w.add_explicit_documents_with_vectors(None, vec![doc], vec![vectors])
                    .expect("add");
            }
        }
        w.commit().expect("commit");
    }
    let reader = DirectoryReader::open(&dir).expect("open");
    let segments = reader.segment_readers();
    assert_eq!(segments.len(), 1, "{}: one segment", path.display());
    // The numbers the documents carry are the engine's: the dump names
    // each field by what the segment says it is.
    let infos: BTreeMap<i32, FieldInfo> = segments[0]
        .field_infos()
        .fields
        .iter()
        .map(|f| (f.number, f.clone()))
        .collect();
    let written: Vec<Value> = infos.values().map(field_json).collect();
    let recorded = &fixture["segment"];
    assert_eq!(
        json!(segments[0].max_doc),
        recorded["max_doc"],
        "{}: max_doc",
        path.display()
    );
    let expected_fields = recorded["fields"].as_array().expect("fields");
    for (i, want) in expected_fields.iter().enumerate() {
        assert_eq!(written.get(i), Some(want), "{}: field {i}", path.display());
    }
    assert_eq!(
        written.len(),
        expected_fields.len(),
        "{}: field count",
        path.display()
    );
    for (i, ((doc, vectors), want)) in docs
        .iter()
        .zip(recorded["docs"].as_array().expect("docs"))
        .enumerate()
    {
        let got = document_json(doc, &infos);
        for part in ["stored", "inverted", "doc_values", "points"] {
            assert_eq!(got[part], want[part], "{}: doc {i}: {part}", path.display());
        }
        let built: BTreeMap<&str, Vec<f64>> = vectors
            .iter()
            .map(|v| {
                let VectorValue::Float32(c) = &v.value else {
                    panic!("a float vector")
                };
                (
                    v.field_name.as_str(),
                    c.iter().map(|x| f64::from(*x)).collect(),
                )
            })
            .collect();
        let recorded_vectors: BTreeMap<&str, Vec<f64>> = want["vectors"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| {
                        (
                            k.as_str(),
                            v.as_array()
                                .expect("vector")
                                .iter()
                                .map(|x| x.as_f64().expect("float"))
                                .collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            built,
            recorded_vectors,
            "{}: doc {i}: vectors",
            path.display()
        );
    }
}

/// Whether a fixture updates or deletes, which only a shard does.
fn has_deletes(path: &Path) -> bool {
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(path).expect("read")).expect("fixture json");
    fixture["documents"]
        .as_array()
        .expect("documents")
        .iter()
        .any(|d| d.get("delete").is_some())
}

#[test]
fn rust_documents_index_as_the_java_indexes_them() {
    let all = fixtures();
    assert!(all.len() >= 3, "fixtures found: {all:?}");
    for path in all.iter().filter(|p| !has_deletes(p)) {
        replay(path.as_path());
    }
}

/// Replays a fixture with updates and deletes through a shard, and compares
/// what the committed segment reads back: its field infos, and each
/// document's stored fields and doc values, `__soft_deletes` among them.
fn replay_through_a_shard(path: &Path) {
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(path).expect("read")).expect("fixture json");
    let mapping = match fixture.get("mappings") {
        Some(m) => Mapping::parse(m.to_string().as_bytes()).expect("mapping"),
        None => Mapping::default(),
    };
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
        for (n, d) in fixture["documents"]
            .as_array()
            .expect("documents")
            .iter()
            .enumerate()
        {
            let id = d["id"].as_str().expect("id");
            if d.get("delete").is_some() {
                shard.delete(id, &Expect::default()).expect("delete");
            } else {
                let source = recorded_source(&fixture, n);
                shard
                    .index(id, source.as_bytes(), false, &Expect::default())
                    .unwrap_or_else(|e| panic!("{}: {id}: {e:?}", path.display()));
            }
        }
        shard.refresh().expect("refresh");
    }
    let reader = DirectoryReader::open(&dir).expect("open");
    let segments = reader.segment_readers();
    assert_eq!(segments.len(), 1, "{}: one segment", path.display());
    let recorded = &fixture["segment"];
    assert_eq!(json!(segments[0].max_doc), recorded["max_doc"]);
    let fields: Vec<Value> = segments[0]
        .field_infos()
        .fields
        .iter()
        .map(field_json)
        .collect();
    assert_eq!(
        Value::Array(fields),
        recorded["fields"],
        "{}: fields",
        path.display()
    );
    for (doc, want) in recorded["docs"]
        .as_array()
        .expect("docs")
        .iter()
        .enumerate()
    {
        let got = read_back(&segments[0], i32::try_from(doc).expect("doc"));
        let want = json!({"stored": want["stored"], "doc_values": want["doc_values"]});
        assert_eq!(got, want, "{}: document {doc}", path.display());
    }
}

#[test]
fn updates_and_deletes_soft_delete_as_the_java_does() {
    let all: Vec<PathBuf> = fixtures().into_iter().filter(|p| has_deletes(p)).collect();
    assert!(!all.is_empty(), "no fixture updates");
    for path in all {
        replay_through_a_shard(&path);
    }
}

#[test]
fn a_stored_field_dump_reads_every_type() {
    let infos: BTreeMap<i32, FieldInfo> = [(0, FieldInfo::new("f", 0))].into();
    let doc = ExplicitDocument {
        stored: vec![
            StoredField {
                field_number: 0,
                value: FieldValue::String("s".into()),
            },
            StoredField {
                field_number: 0,
                value: FieldValue::Long(2),
            },
        ],
        fields: Default::default(),
    };
    assert_eq!(
        document_json(&doc, &infos)["stored"],
        json!([["f", "string", "s"], ["f", "long", 2]])
    );
}
