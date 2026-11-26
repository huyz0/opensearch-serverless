//! docs/specs/nested-fields AC-6: a document with nested objects as the
//! block of Lucene documents the engine writes.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::reader::LeafReader;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::lucene::{Indexer, Meta};
use oss_doc::mapping::{Mapping, ParseOptions};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const MAPPING: &str = r#"{"properties":{"title":{"type":"keyword"},"c":{"type":"nested","properties":{"w":{"type":"keyword"},"d":{"type":"nested","properties":{"x":{"type":"long"}}}}}}}"#;

fn mapping() -> Mapping {
    let on = ParseOptions {
        nested: true,
        ..ParseOptions::default()
    };
    Mapping::parse_with(MAPPING.as_bytes(), on).expect("mapping")
}

/// The directory after `id` was added as one block, committed.
fn block(id: &str, source: &str) -> ByteBuffersDirectory {
    let m = mapping();
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut indexer = Indexer::new();
        let parsed = document::parse(&m, id, source.as_bytes()).expect("parse");
        let meta = Meta {
            id,
            seq_no: 7,
            primary_term: 3,
            version: 1,
            source: source.as_bytes(),
        };
        let docs = indexer
            .block_routed(&mut w, &m, &parsed, &meta, None)
            .expect("block");
        w.add_explicit_documents(docs).expect("add");
        w.commit().expect("commit");
    }
    dir
}

/// Whether each document of the one segment has a value of the numeric
/// doc values `field`.
fn has_numeric(dir: &ByteBuffersDirectory, field: &str) -> Vec<bool> {
    let reader = DirectoryReader::open(dir).expect("open");
    let segment = reader.segment_readers().first().expect("a segment");
    let mut dv = segment
        .numeric_doc_values(field)
        .expect("reads")
        .expect("doc values");
    (0..segment.max_doc)
        .map(|d| dv.advance_exact(d).expect("advances"))
        .collect()
}

fn freq(dir: &ByteBuffersDirectory, field: &str, term: &str) -> i32 {
    let reader = DirectoryReader::open(dir).expect("open");
    reader
        .segment_readers()
        .iter()
        .map(|s| s.doc_freq(field, term.as_bytes()).expect("doc freq"))
        .sum()
}

#[test]
fn a_block_is_the_children_in_order_and_then_the_document() {
    let dir = block(
        "1",
        r#"{"title":"t","c":[{"w":"a","d":[{"x":1},{"x":2}]},{"w":"b"}]}"#,
    );
    let reader = DirectoryReader::open(&dir).expect("open");
    // c.d, c.d, c, c, and the document.
    assert_eq!(reader.max_doc(), 5);
    assert_eq!(freq(&dir, "_nested_path", "c.d"), 2);
    assert_eq!(freq(&dir, "_nested_path", "c"), 2);
    // Each child's own fields.
    assert_eq!(freq(&dir, "c.w", "a"), 1);
    assert_eq!(freq(&dir, "c.w", "b"), 1);
    assert_eq!(freq(&dir, "title", "t"), 1);
    // `_id` is on all five, so a delete of the id reaches them all.
    let id = oss_doc::lucene::uid("1").expect("a key");
    assert_eq!(
        reader
            .segment_readers()
            .first()
            .expect("a segment")
            .doc_freq("_id", &id)
            .expect("doc freq"),
        5
    );
}

#[test]
fn only_the_document_has_its_metadata() {
    let dir = block("1", r#"{"title":"t","c":[{"w":"a"},{"w":"b"}]}"#);
    let last = [false, false, true];
    assert_eq!(has_numeric(&dir, "_primary_term"), last);
    assert_eq!(has_numeric(&dir, "_seq_no"), last);
    assert_eq!(has_numeric(&dir, "_version"), last);
}

#[test]
fn a_document_without_nested_objects_is_the_one_document_it_was() {
    let dir = block("1", r#"{"title":"t"}"#);
    let reader = DirectoryReader::open(&dir).expect("open");
    assert_eq!(reader.max_doc(), 1);
    assert_eq!(freq(&dir, "_nested_path", "c"), 0);
}

// AC-8: the plan reads the fields of the document it knows, and knows no nested
// object, so an index with one never takes the plan.
#[test]
fn an_index_with_a_nested_object_never_takes_the_fast_plan() {
    let source = br#"{"title":"t"}"#;
    let key = oss_doc::lucene::uid("2").expect("a key");
    let plain = Mapping::parse(br#"{"properties":{"title":{"type":"keyword"}}}"#).expect("plain");
    let nested = mapping();
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut indexer = Indexer::new();
    for m in [&plain, &nested] {
        // Number the fields as a first document does.
        let parsed = document::parse(m, "1", source).expect("parse");
        let meta = Meta {
            id: "1",
            seq_no: 0,
            primary_term: 1,
            version: 1,
            source,
        };
        let docs = indexer
            .block_routed(&mut w, m, &parsed, &meta, None)
            .expect("block");
        w.add_explicit_documents(docs).expect("add");
        let taken = oss_doc::fast::FastIngest::new()
            .prepare(&indexer, m, source, &key)
            .is_some();
        assert_eq!(taken, !m.has_nested(), "nested: {}", m.has_nested());
    }
}
