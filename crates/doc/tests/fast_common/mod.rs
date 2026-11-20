//! What the fast-ingest tests share: a writer and indexer per side, and the comparison of the
//! plan against the complete path.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]
// Each test file uses some of these.
#![allow(dead_code)]

use lucene_index::index_writer::{ExplicitDocument, IndexWriter, InvertedField};
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::fast::FastIngest;
use oss_doc::lucene::{Indexer, Meta, uid};
use oss_doc::mapping::Mapping;

pub const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// A writer and the indexer numbering for it.
pub struct Side<'d> {
    pub writer: IndexWriter<'d>,
    pub indexer: Indexer,
    pub mapping: Mapping,
}

pub fn side<'d>(dir: &'d ByteBuffersDirectory, mapping: &str) -> Side<'d> {
    let mut writer = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    writer.enable_explicit_documents().expect("explicit");
    Side {
        writer,
        indexer: Indexer::new(),
        mapping: Mapping::parse(mapping.as_bytes()).expect("mapping"),
    }
}

impl Side<'_> {
    /// The complete path.
    pub fn slow(&mut self, id: &str, seq: i64, source: &[u8]) -> Option<ExplicitDocument> {
        let parsed = document::parse(&self.mapping, id, source).ok()?;
        if let Some(grown) = &parsed.grown {
            self.mapping = grown.clone();
        }
        let meta = Meta {
            id,
            seq_no: seq,
            primary_term: 1,
            version: 1,
            source,
        };
        Some(
            self.indexer
                .document(&mut self.writer, &self.mapping, &parsed, &meta)
                .expect("document"),
        )
    }
}

/// One entry list, ordered by field number (entries of one field keep their order).
fn by_number<T: Clone>(v: &[T], number: impl Fn(&T) -> i32) -> Vec<T> {
    let mut out = v.to_vec();
    out.sort_by_key(|e| number(e));
    out
}

/// A field's terms in term order; a term without positions is a document-only occurrence, whose
/// frequency the segment does not keep.
fn terms_of(f: &InvertedField) -> Vec<String> {
    let mut out: Vec<String> = f
        .terms
        .iter()
        .map(|t| {
            let freq = if t.positions.is_empty() { 0 } else { t.freq };
            format!(
                "{:?} freq {freq} pos {:?} off {:?} pay {}",
                t.term,
                t.positions,
                t.offsets,
                t.payloads.len()
            )
        })
        .collect();
    out.sort();
    out
}

pub fn canonical(d: &ExplicitDocument) -> String {
    let stored = by_number(&d.stored, |s| s.field_number);
    let inv = by_number(&d.fields.inverted, |s| s.field_number);
    let dv = by_number(&d.fields.doc_values, |s| s.field_number);
    let pt = by_number(&d.fields.points, |s| s.field_number);
    let inverted: Vec<String> = inv
        .iter()
        .map(|f| format!("{} norm {:?} {:?}", f.field_number, f.norm, terms_of(f)))
        .collect();
    let mut registered = d.fields.registered.clone();
    registered.sort_unstable();
    format!(
        "stored {stored:?}\ninverted {inverted:?}\ndv {dv:?}\npoints {pt:?}\nregistered {registered:?}"
    )
}

/// Indexes `sources` on both sides; whether each was read by the plan.
pub fn run(mapping: &str, sources: &[&str]) -> Vec<bool> {
    let (d1, d2) = (ByteBuffersDirectory::new(), ByteBuffersDirectory::new());
    let (mut slow, mut fast) = (side(&d1, mapping), side(&d2, mapping));
    let mut path = FastIngest::new();
    let mut taken = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let (id, seq) = (format!("id{i}"), i as i64);
        let key = uid(&id).expect("uid");
        let Some(want) = slow.slow(&id, seq, source.as_bytes()) else {
            // The complete path refuses it: the plan must leave it to the complete path.
            assert!(
                path.prepare(&fast.indexer, &fast.mapping, source.as_bytes(), &key)
                    .is_none(),
                "document {i} is an error and the plan read it: {source}"
            );
            taken.push(false);
            continue;
        };
        let got = match path.prepare(&fast.indexer, &fast.mapping, source.as_bytes(), &key) {
            Some(f) => {
                taken.push(true);
                f.finish(seq, 1, 1).pop().expect("one document")
            }
            None => {
                taken.push(false);
                let before = fast.mapping.clone();
                let d = fast
                    .slow(&id, seq, source.as_bytes())
                    .expect("same as slow");
                if fast.mapping != before {
                    path.invalidate();
                }
                d
            }
        };
        let (got, want) = (canonical(&got), canonical(&want));
        if got != want {
            let first = got.lines().zip(want.lines()).position(|(a, b)| a != b);
            panic!(
                "document {i}: {source}\nfirst differing section: {:?}\nfast: {}\nslow: {}",
                first.map(|l| got.lines().nth(l).map(|x| &x[..x.len().min(600)])),
                got.lines().nth(first.unwrap_or(0)).unwrap_or(""),
                want.lines().nth(first.unwrap_or(0)).unwrap_or("")
            );
        }
    }
    taken
}

pub const LOGS: &str = r#"{"properties":{
  "@timestamp":{"type":"date"},"level":{"type":"keyword"},"host":{"type":"keyword"},
  "status":{"type":"integer"},"bytes":{"type":"long"},"message":{"type":"text"}}}"#;

pub fn log(i: usize) -> String {
    format!(
        r#"{{"@timestamp":"2026-11-20T08:{:02}:{:02}.000Z","level":"info","host":"web-{}","status":{},"bytes":{},"message":"GET /api/v1/items/{} returned ok after {} ms"}}"#,
        (i / 60) % 60,
        i % 60,
        i % 50,
        200 + (i % 5),
        i * 13,
        i,
        i % 900
    )
}
