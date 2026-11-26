//! docs/specs/nested-fields AC-7: a shard indexes a document with nested
//! objects as one block, replaces the whole block on an update and removes
//! it on a delete.

#![allow(clippy::disallowed_methods)]
// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::reader::LeafReader;
use lucene_store::ByteBuffersDirectory;
use oss_format::WalRecord;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

const MAPPING: &str = r#"{"properties":{"title":{"type":"keyword"},"c":{"type":"nested","properties":{"w":{"type":"keyword"}}}}}"#;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// The `_nested_path` and `w` of every live document of the directory, in
/// document order: a child has a path, the document none.
fn live(dir: &ByteBuffersDirectory) -> Vec<bool> {
    let reader = DirectoryReader::open(dir).expect("open");
    let mut out = Vec::new();
    for segment in reader.segment_readers() {
        let mut deleted = segment.numeric_doc_values("__soft_deletes").expect("reads");
        let mut is_root = segment.numeric_doc_values("_primary_term").expect("reads");
        for doc in 0..segment.max_doc {
            let gone = match deleted.as_mut() {
                Some(dv) => dv.advance_exact(doc).expect("advances"),
                None => false,
            };
            if !gone {
                let root = match is_root.as_mut() {
                    Some(dv) => dv.advance_exact(doc).expect("advances"),
                    None => false,
                };
                out.push(root);
            }
        }
    }
    out
}

fn index(shard: &mut Shard<'_>, id: &str, source: &str) {
    shard
        .index(id, source.as_bytes(), false, &Expect::default())
        .unwrap_or_else(|e| panic!("{id}: {e:?}"));
}

#[test]
fn a_document_is_a_block_an_update_replaces_it_and_a_delete_removes_it() {
    let on = ParseOptions {
        nested: true,
        ..ParseOptions::default()
    };
    let mapping = Mapping::parse_with(MAPPING.as_bytes(), on).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
        index(
            &mut shard,
            "1",
            r#"{"title":"a","c":[{"w":"x"},{"w":"y"}]}"#,
        );
        index(&mut shard, "2", r#"{"title":"b","c":[{"w":"z"}]}"#);
        shard.refresh().expect("refresh");
        // The children, then the document, a block each.
        assert_eq!(live(&dir), [false, false, true, false, true], "two blocks");
        // The realtime get finds the document, never a child.
        let got = shard.get("1", true).expect("gets").expect("found");
        assert_eq!(
            String::from_utf8(got.source).expect("text"),
            r#"{"title":"a","c":[{"w":"x"},{"w":"y"}]}"#
        );
        assert_eq!(got.version, 1);

        // An update with one child leaves one child and the document.
        index(&mut shard, "1", r#"{"title":"a2","c":[{"w":"q"}]}"#);
        shard.refresh().expect("refresh");
        let after_update = live(&dir);
        assert_eq!(
            after_update.iter().filter(|r| **r).count(),
            2,
            "{after_update:?}"
        );
        assert_eq!(after_update.len(), 4, "{after_update:?}");
        let got = shard.get("1", true).expect("gets").expect("found");
        assert_eq!(got.version, 2);
        assert!(String::from_utf8(got.source).expect("text").contains("a2"));

        // An update with no nested object leaves the document alone.
        index(&mut shard, "1", r#"{"title":"a3"}"#);
        shard.refresh().expect("refresh");
        assert_eq!(live(&dir).len(), 3);

        // A delete takes the document and the block.
        shard.delete("2", &Expect::default()).expect("delete");
        shard.refresh().expect("refresh");
        assert_eq!(live(&dir), [true]);
        assert!(shard.get("2", true).expect("gets").is_none());
    }
}

#[test]
fn a_document_that_fails_to_parse_writes_no_child() {
    let on = ParseOptions {
        nested: true,
        ..ParseOptions::default()
    };
    let mapping = Mapping::parse_with(MAPPING.as_bytes(), on).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
    let bad = shard.index(
        "1",
        br#"{"c":[{"w":"x"},{"w":{"not":"a keyword"}}]}"#,
        false,
        &Expect::default(),
    );
    assert!(bad.is_err());
    index(&mut shard, "2", r#"{"c":[{"w":"x"}]}"#);
    shard.refresh().expect("refresh");
    assert_eq!(live(&dir), [false, true]);
}

// A takeover replays the predecessor's records: a nested document comes back
// as its block, and an update replaces the whole block.
#[test]
fn a_replayed_record_is_a_block_and_a_replayed_update_replaces_it() {
    let on = ParseOptions {
        nested: true,
        ..ParseOptions::default()
    };
    let mapping = Mapping::parse_with(MAPPING.as_bytes(), on).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
    let record = |source: &str, seq: i64, version: i64| {
        WalRecord::index("1".to_owned(), source.to_owned(), seq, 1, version)
    };
    assert!(
        shard
            .replay(&record(r#"{"title":"a","c":[{"w":"x"},{"w":"y"}]}"#, 0, 1))
            .expect("replays")
    );
    shard.refresh().expect("refresh");
    assert_eq!(live(&dir), [false, false, true]);
    assert!(
        shard
            .replay(&record(r#"{"title":"b","c":[{"w":"q"}]}"#, 1, 2))
            .expect("replays")
    );
    shard.refresh().expect("refresh");
    assert_eq!(live(&dir), [false, true]);
}
