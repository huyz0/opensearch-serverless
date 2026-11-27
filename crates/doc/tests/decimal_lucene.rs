//! `half_float` and `scaled_float` as Lucene fields: a half float is a
//! 2-byte point with its sortable short as doc value, a scaled float a long
//! point with the scaled long (`HalfFloatPoint`, `NumberType.LONG`).

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::reader::LeafReader;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::lucene::{Indexer, Meta};
use oss_doc::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const MAPPING: &str = r#"{"properties":{"h":{"type":"half_float"},"s":{"type":"scaled_float","scaling_factor":100}}}"#;
const SOURCE: &str = r#"{"h":1.5,"s":2.345}"#;

fn directory() -> ByteBuffersDirectory {
    let m = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut indexer = Indexer::new();
        let parsed = document::parse(&m, "1", SOURCE.as_bytes()).expect("parse");
        let meta = Meta {
            id: "1",
            seq_no: 0,
            primary_term: 1,
            version: 1,
            source: SOURCE.as_bytes(),
        };
        let doc = indexer.document(&mut w, &m, &parsed, &meta).expect("doc");
        w.add_explicit_documents(vec![doc]).expect("add");
        w.commit().expect("commit");
    }
    dir
}

fn doc_value(dir: &ByteBuffersDirectory, field: &str) -> i64 {
    let reader = DirectoryReader::open(dir).expect("open");
    let segment = reader.segment_readers().first().expect("a segment");
    let mut dv = segment
        .sorted_numeric_doc_values(field)
        .expect("reads")
        .expect("doc values");
    assert!(dv.advance_exact(0).expect("advances"));
    assert_eq!(dv.doc_value_count(), 1);
    dv.next_value().expect("a value")
}

#[test]
fn a_half_float_is_a_two_byte_point_with_a_sortable_short() {
    let dir = directory();
    // `HalfFloatPoint.halfFloatToSortableShort(1.5f)`.
    assert_eq!(doc_value(&dir, "h"), 0x3e00);
    let reader = DirectoryReader::open(&dir).expect("open");
    let segment = reader.segment_readers().first().expect("a segment");
    let info = segment.field_infos().field_by_name("h").expect("field h");
    assert_eq!((info.point_dimension_count, info.point_num_bytes), (1, 2));
}

#[test]
fn a_scaled_float_is_a_long_point_with_the_scaled_long() {
    let dir = directory();
    // `Math.round(2.345 * 100)`: the product is 234.50000000000003 in binary, so 235.
    assert_eq!(doc_value(&dir, "s"), 235);
    let reader = DirectoryReader::open(&dir).expect("open");
    let segment = reader.segment_readers().first().expect("a segment");
    let info = segment.field_infos().field_by_name("s").expect("field s");
    assert_eq!((info.point_dimension_count, info.point_num_bytes), (1, 8));
}
