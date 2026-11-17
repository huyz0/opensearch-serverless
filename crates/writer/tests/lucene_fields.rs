//! Field numbers across segments and writers, and a mapping update the
//! engine's schema cannot take: what one fixture segment cannot show.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_writer::document;
use oss_writer::lucene::{Indexer, Meta};
use oss_writer::mapping::{Mapping, MappingError};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn mapping(json: &str) -> Mapping {
    Mapping::parse(json.as_bytes()).expect("mapping")
}

/// Indexes `source` as document `id` through `indexer` into `w`.
fn index(
    w: &mut IndexWriter<'_>,
    indexer: &mut Indexer,
    mapping: &Mapping,
    id: &str,
    source: &str,
) -> Result<(), MappingError> {
    let parsed = document::parse(mapping, id, source.as_bytes()).expect("parse");
    let meta = Meta {
        id,
        seq_no: 0,
        primary_term: 1,
        version: 1,
        source: source.as_bytes(),
    };
    let doc = indexer.document(w, mapping, &parsed, &meta)?;
    w.add_explicit_documents(vec![doc]).expect("add");
    Ok(())
}

/// Each segment's document frequency of `term` in `field`.
fn doc_freqs(dir: &ByteBuffersDirectory, field: &str, term: &str) -> Vec<i32> {
    let reader = DirectoryReader::open(dir).expect("open");
    reader
        .segment_readers()
        .iter()
        .map(|s| s.doc_freq(field, term.as_bytes()).expect("doc freq"))
        .collect()
}

#[test]
fn a_later_segment_keeps_the_writers_field_numbers() {
    let m = mapping(r#"{"properties":{"a":{"type":"keyword"},"b":{"type":"keyword"}}}"#);
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut indexer = Indexer::new();
        index(&mut w, &mut indexer, &m, "1", r#"{"a":"x","b":"y"}"#).unwrap();
        w.commit().expect("commit");
        // The second segment meets `b` first; it keeps `b`'s number.
        index(&mut w, &mut indexer, &m, "2", r#"{"b":"z"}"#).unwrap();
        w.commit().expect("commit");
    }
    {
        // A new writer over the index, with a new indexer, learns the
        // numbers from the segments.
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut indexer = Indexer::new();
        index(&mut w, &mut indexer, &m, "3", r#"{"b":"w","a":"v"}"#).unwrap();
        w.commit().expect("commit");
    }
    assert_eq!(doc_freqs(&dir, "b", "z"), [0, 1, 0]);
    assert_eq!(doc_freqs(&dir, "a", "z"), [0, 0, 0]);
    assert_eq!(doc_freqs(&dir, "b", "w"), [0, 0, 1]);
    assert_eq!(doc_freqs(&dir, "a", "v"), [0, 0, 1]);
    let reader = DirectoryReader::open(&dir).expect("open");
    let number = |seg: usize, name: &str| {
        reader.segment_readers()[seg]
            .field_infos()
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.number)
    };
    for seg in 1..3 {
        assert_eq!(number(seg, "b"), number(0, "b"));
    }
}

#[test]
fn a_mapping_update_the_schema_cannot_take_is_refused() {
    let before = mapping(r#"{"properties":{"title":{"type":"text"}}}"#);
    let after = before
        .merge(&mapping(
            r#"{"properties":{"title":{"type":"text","norms":false}}}"#,
        ))
        .expect("norms may be turned off");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut indexer = Indexer::new();
    index(&mut w, &mut indexer, &before, "1", r#"{"title":"a b"}"#).unwrap();
    let e = index(&mut w, &mut indexer, &after, "2", r#"{"title":"c d"}"#).unwrap_err();
    assert_eq!(e.ty, "illegal_argument_exception");
    assert!(
        e.reason.starts_with(r#"cannot change field "title" from "#),
        "{e:?}"
    );
    // The field keeps its schema: the old mapping still indexes.
    index(&mut w, &mut indexer, &before, "3", r#"{"title":"e"}"#).unwrap();
}
