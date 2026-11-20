//! Each fixture of `fixtures/segments/` holds a mapping, documents,
//! and the segment stock OpenSearch wrote for them, as `SegmentDump.java`
//! reads it. The writer parses the same documents, builds their Lucene
//! documents, writes them with the engine, and must dump the same: field
//! infos as the engine wrote them, and each document's stored fields, terms,
//! norms, doc values and points as handed to the engine.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
// A fixture is a small file the test reads whole.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lucene_codecs::field_infos::{DocValuesSkipIndexType, DocValuesType, FieldInfo, IndexOptions};
use lucene_codecs::stored_fields::{FieldValue, StoredField};
use lucene_index::index_writer::{ExplicitDocument, IndexWriter};
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::reader::LeafReader;
use lucene_store::ByteBuffersDirectory;
use oss_writer::document;
use oss_writer::index::{Expect, Shard};
use oss_writer::lucene::{Indexer, Meta};
use oss_writer::mapping::Mapping;
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

/// Bytes as `SegmentDump.bytes` writes them.
fn bytes(b: &[u8]) -> Value {
    if let Ok(s) = std::str::from_utf8(b)
        && !s.chars().any(|c| c < ' ' || c == '\u{7f}')
        && !s.starts_with("hex:")
    {
        return json!(s);
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    json!(format!("hex:{hex}"))
}

fn index_options(o: IndexOptions) -> &'static str {
    match o {
        IndexOptions::None => "NONE",
        IndexOptions::Docs => "DOCS",
        IndexOptions::DocsAndFreqs => "DOCS_AND_FREQS",
        IndexOptions::DocsAndFreqsAndPositions => "DOCS_AND_FREQS_AND_POSITIONS",
        IndexOptions::DocsAndFreqsAndPositionsAndOffsets => {
            "DOCS_AND_FREQS_AND_POSITIONS_AND_OFFSETS"
        }
        IndexOptions::DocsAndCustomFreqs => "DOCS_AND_CUSTOM_FREQS",
    }
}

fn doc_values(t: DocValuesType) -> &'static str {
    match t {
        DocValuesType::None => "NONE",
        DocValuesType::Numeric => "NUMERIC",
        DocValuesType::Binary => "BINARY",
        DocValuesType::Sorted => "SORTED",
        DocValuesType::SortedNumeric => "SORTED_NUMERIC",
        DocValuesType::SortedSet => "SORTED_SET",
    }
}

fn field_json(f: &FieldInfo) -> Value {
    json!({
        "number": f.number,
        "name": f.name,
        "index_options": index_options(f.index_options),
        "doc_values": doc_values(f.doc_values_type),
        "skip_index": match f.doc_values_skip_index_type {
            DocValuesSkipIndexType::None => "NONE",
            DocValuesSkipIndexType::Range => "RANGE",
        },
        "omit_norms": f.omit_norms,
        "points": [f.point_dimension_count, f.point_index_dimension_count, f.point_num_bytes],
        "payloads": f.store_payloads,
        "term_vectors": f.store_term_vectors,
    })
}

fn stored_json(name: &str, v: &FieldValue) -> Value {
    match v {
        FieldValue::Binary(b) => json!([name, "binary", bytes(b)]),
        FieldValue::String(s) => json!([name, "string", s]),
        FieldValue::Shared(s) => json!([name, "string", s.as_str()]),
        FieldValue::Int(i) => json!([name, "int", i]),
        FieldValue::Long(l) => json!([name, "long", l]),
        FieldValue::Float(f) => json!([name, "float", java_float(*f)]),
        FieldValue::Double(d) => {
            json!([name, "double", oss_writer::mapping::json::java_double(*d)])
        }
    }
}

fn java_float(f: f32) -> String {
    oss_writer::mapping::json::java_float(f)
}

/// One document as `SegmentDump.document` writes it.
fn document_json(doc: &ExplicitDocument, infos: &BTreeMap<i32, FieldInfo>) -> Value {
    let name = |n: i32| infos.get(&n).expect("registered").name.clone();
    let stored: Vec<Value> = doc
        .stored
        .iter()
        .map(|f| stored_json(&name(f.field_number), &f.value))
        .collect();
    let mut inverted = serde_json::Map::new();
    for f in &doc.fields.inverted {
        let info = &infos[&f.field_number];
        let positions = info.index_options.subsumes_positions();
        let offsets = info.index_options.subsumes_offsets();
        let freqs = !matches!(info.index_options, IndexOptions::None | IndexOptions::Docs);
        let mut terms: Vec<_> = f.terms.iter().collect();
        if terms.is_empty() {
            continue;
        }
        terms.sort_by(|a, b| a.term.cmp(b.term));
        let terms: Vec<Value> = terms
            .iter()
            .map(|t| {
                let mut e = vec![bytes(t.term), json!(if freqs { t.freq } else { 1 })];
                if positions {
                    e.push(json!(t.positions));
                }
                if offsets {
                    e.push(json!(
                        t.offsets.iter().map(|(s, e)| [*s, *e]).collect::<Vec<_>>()
                    ));
                }
                Value::Array(e)
            })
            .collect();
        let norm = f.norm.filter(|n| *n != 0);
        inverted.insert(name(f.field_number), json!({"norm": norm, "terms": terms}));
    }
    let mut dv: BTreeMap<String, Vec<&FieldValue>> = BTreeMap::new();
    for f in &doc.fields.doc_values {
        dv.entry(name(f.field_number)).or_default().push(&f.value);
    }
    let mut doc_values_out = serde_json::Map::new();
    for (field, values) in dv {
        let info = infos.values().find(|i| i.name == field).expect("field");
        let v = match info.doc_values_type {
            DocValuesType::Numeric => match values.as_slice() {
                [FieldValue::Long(l)] => json!(l),
                other => panic!("{field}: one numeric value, not {other:?}"),
            },
            DocValuesType::SortedNumeric => {
                let mut longs: Vec<i64> = values
                    .iter()
                    .map(|v| match v {
                        FieldValue::Long(l) => *l,
                        other => panic!("sorted numeric {other:?}"),
                    })
                    .collect();
                longs.sort_unstable();
                json!(longs)
            }
            DocValuesType::SortedSet => {
                let mut set: Vec<&[u8]> = values
                    .iter()
                    .map(|v| match v {
                        FieldValue::Binary(b) => b.as_slice(),
                        other => panic!("sorted set {other:?}"),
                    })
                    .collect();
                set.sort_unstable();
                set.dedup();
                Value::Array(set.into_iter().map(bytes).collect())
            }
            other => panic!("doc values {other:?}"),
        };
        doc_values_out.insert(field, v);
    }
    let mut points: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut packed: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    for f in &doc.fields.points {
        match &f.value {
            FieldValue::Binary(b) => packed
                .entry(name(f.field_number))
                .or_default()
                .push(b.clone()),
            other => panic!("point {other:?}"),
        }
    }
    for (field, mut values) in packed {
        values.sort();
        points.insert(field, values.iter().map(|v| bytes(v)).collect());
    }
    json!({
        "stored": stored,
        "inverted": inverted,
        "doc_values": doc_values_out,
        "points": points,
    })
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
    let mut mapping = match fixture.get("mappings") {
        Some(m) => Mapping::parse(m.to_string().as_bytes()).expect("mapping"),
        None => Mapping::default(),
    };
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
            let doc = indexer
                .document(&mut w, &mapping, &parsed, &meta)
                .unwrap_or_else(|e| panic!("{}: {id}: {e:?}", path.display()));
            docs.push(doc.clone());
            w.add_explicit_documents(vec![doc]).expect("add");
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
    for (i, (doc, want)) in docs
        .iter()
        .zip(recorded["docs"].as_array().expect("docs"))
        .enumerate()
    {
        let got = document_json(doc, &infos);
        for part in ["stored", "inverted", "doc_values", "points"] {
            assert_eq!(got[part], want[part], "{}: doc {i}: {part}", path.display());
        }
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

/// One committed document's stored fields and doc values, read back from
/// the segment in `SegmentDump.document`'s form.
fn read_back(segment: &lucene_search::directory_reader::SegmentReader, doc: i32) -> Value {
    let infos: BTreeMap<i32, FieldInfo> = segment
        .field_infos()
        .fields
        .iter()
        .map(|f| (f.number, f.clone()))
        .collect();
    let stored: Vec<Value> = segment
        .stored_document(doc)
        .expect("stored")
        .map(|d| {
            d.fields
                .iter()
                .map(|f| stored_json(&infos[&f.field_number].name, &f.value))
                .collect()
        })
        .unwrap_or_default();
    let mut dv = serde_json::Map::new();
    for info in infos.values() {
        let v = match info.doc_values_type {
            DocValuesType::Numeric => {
                let mut it = segment
                    .numeric_doc_values(&info.name)
                    .expect("dv")
                    .expect("field");
                it.advance_exact(doc)
                    .expect("advance")
                    .then(|| json!(it.long_value()))
            }
            DocValuesType::SortedNumeric => {
                let mut it = segment
                    .sorted_numeric_doc_values(&info.name)
                    .expect("dv")
                    .expect("field");
                it.advance_exact(doc).expect("advance").then(|| {
                    let n = it.doc_value_count();
                    json!(
                        (0..n)
                            .map(|_| it.next_value().expect("value"))
                            .collect::<Vec<_>>()
                    )
                })
            }
            DocValuesType::SortedSet => {
                let mut it = segment
                    .sorted_set_doc_values(&info.name)
                    .expect("dv")
                    .expect("field");
                it.advance_exact(doc).expect("advance").then(|| {
                    let n = it.doc_value_count();
                    let ords: Vec<i64> = (0..n).map(|_| it.next_ord().expect("ord")).collect();
                    Value::Array(
                        ords.into_iter()
                            .map(|o| bytes(&it.lookup_ord(o).expect("term")))
                            .collect(),
                    )
                })
            }
            _ => None,
        };
        if let Some(v) = v {
            dv.insert(info.name.clone(), v);
        }
    }
    json!({"stored": stored, "doc_values": dv})
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
