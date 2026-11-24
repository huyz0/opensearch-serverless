//! The stored form of a segment's fields and documents, as
//! `fixtures/segments/SegmentDump.java` writes it, shared by the segment
//! fixture tests.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::collections::BTreeMap;

use lucene_codecs::field_infos::{
    DocValuesSkipIndexType, DocValuesType, FieldInfo, IndexOptions, VectorSimilarityFunction,
};
use lucene_codecs::stored_fields::FieldValue;
use lucene_index::index_writer::ExplicitDocument;
use lucene_search::reader::LeafReader;
use serde_json::{Value, json};

/// Bytes as `SegmentDump.bytes` writes them.
pub fn bytes(b: &[u8]) -> Value {
    if let Ok(s) = std::str::from_utf8(b)
        && !s.chars().any(|c| c < ' ' || c == '\u{7f}')
        && !s.starts_with("hex:")
    {
        return json!(s);
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    json!(format!("hex:{hex}"))
}

pub fn index_options(o: IndexOptions) -> &'static str {
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

pub fn doc_values(t: DocValuesType) -> &'static str {
    match t {
        DocValuesType::None => "NONE",
        DocValuesType::Numeric => "NUMERIC",
        DocValuesType::Binary => "BINARY",
        DocValuesType::Sorted => "SORTED",
        DocValuesType::SortedNumeric => "SORTED_NUMERIC",
        DocValuesType::SortedSet => "SORTED_SET",
    }
}

/// `VectorSimilarityFunction.toString()`.
pub fn similarity(f: VectorSimilarityFunction) -> &'static str {
    match f {
        VectorSimilarityFunction::Euclidean => "EUCLIDEAN",
        VectorSimilarityFunction::DotProduct => "DOT_PRODUCT",
        VectorSimilarityFunction::Cosine => "COSINE",
        VectorSimilarityFunction::MaximumInnerProduct => "MAXIMUM_INNER_PRODUCT",
    }
}

pub fn field_json(f: &FieldInfo) -> Value {
    let mut out = field_base(f);
    if f.vector_dimension > 0 {
        let mut attributes: Vec<_> = f.attributes.clone();
        attributes.sort();
        out["vector"] = json!({
            "dimension": f.vector_dimension,
            "encoding": "FLOAT32",
            "similarity": similarity(f.vector_similarity_function),
            "attributes": attributes.into_iter().collect::<BTreeMap<_, _>>(),
        });
    }
    out
}

pub fn field_base(f: &FieldInfo) -> Value {
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

pub fn stored_json(name: &str, v: &FieldValue) -> Value {
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

pub fn java_float(f: f32) -> String {
    oss_writer::mapping::json::java_float(f)
}

/// One document as `SegmentDump.document` writes it.
pub fn document_json(doc: &ExplicitDocument, infos: &BTreeMap<i32, FieldInfo>) -> Value {
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

/// One committed document's stored fields and doc values, read back from
/// the segment in `SegmentDump.document`'s form.
pub fn read_back(segment: &lucene_search::directory_reader::SegmentReader, doc: i32) -> Value {
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
