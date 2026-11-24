//! docs/specs/knn-derived-source: the segment a shard over the knn engine
//! writes stores `_source` as the stock plugin's does, lists its vector
//! fields as derived, and reads the vectors back into `_source`.

#![allow(clippy::disallowed_methods)]

use std::path::Path;

use lucene_codecs::stored_fields::{StoredFieldVisitor, VisitStatus};
use lucene_search::directory_reader::{DirectoryReader, SegmentReader};
use lucene_store::{ByteBuffersDirectory, FsDirectory};
use oss_writer::index::knn::{KNN_CODEC, open_writer};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};
use serde_json::Value;

/// The raw stored `_source` of each document of the `knn-*` fixtures, read
/// from the segment the stock plugin wrote.
const STOCK_RAW: [&str; 6] = [
    r#"{"v":1,"k":"a"}"#,
    r#"{"v":1,"k":"b"}"#,
    r#"{"k":"no vector"}"#,
    r#"{"o":{"v":1},"k":"nested"}"#,
    r#"{"v":1}"#,
    r#"{"v":null,"k":"null vector"}"#,
];

/// The same documents as a read puts their vectors back.
const REBUILT: [&str; 6] = [
    r#"{"v":[1.0,2.0,3.0],"k":"a"}"#,
    r#"{"v":[0.5,-1.25,300.0],"k":"b"}"#,
    r#"{"k":"no vector"}"#,
    r#"{"o":{"v":[7.0,8.0]},"k":"nested"}"#,
    r#"{"v":[1.0,2.0,3.0]}"#,
    r#"{"v":null,"k":"null vector"}"#,
];

struct Source {
    field: Option<i32>,
    bytes: Vec<u8>,
}

impl StoredFieldVisitor for Source {
    fn needs_field(&mut self, number: i32) -> lucene_codecs::stored_fields::Result<VisitStatus> {
        Ok(if Some(number) == self.field {
            VisitStatus::Yes
        } else {
            VisitStatus::No
        })
    }

    fn binary_field(
        &mut self,
        _number: i32,
        value: &[u8],
    ) -> lucene_codecs::stored_fields::Result<()> {
        self.bytes = value.to_vec();
        Ok(())
    }
}

/// `(raw, as read)` `_source` of document `doc`.
fn sources(segment: &SegmentReader, doc: i32) -> (String, String) {
    let number = segment
        .field_infos()
        .field_by_name("_source")
        .map(|f| f.number);
    let raw = segment
        .stored_document(doc)
        .expect("stored")
        .expect("a document")
        .fields
        .into_iter()
        .find_map(|f| match f.value {
            lucene_codecs::stored_fields::FieldValue::Binary(b)
                if Some(f.field_number) == number =>
            {
                Some(b)
            }
            _ => None,
        })
        .expect("a _source");
    let mut read = Source {
        field: number,
        bytes: Vec::new(),
    };
    assert!(
        segment
            .visit_source_document(doc, &mut read)
            .expect("visit")
    );
    (
        String::from_utf8(raw).expect("utf8"),
        String::from_utf8(read.bytes).expect("utf8"),
    )
}

fn recorded_source(fixture: &Value, doc: usize) -> String {
    let stored = fixture["segment"]["docs"][doc]["stored"]
        .as_array()
        .expect("stored");
    let source = stored.iter().find(|f| f[0] == "_source").expect("_source");
    source[2].as_str().expect("text").to_owned()
}

fn fixture(name: &str) -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/segments/{name}.json"));
    serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json")
}

// AC-1
#[test]
fn a_stock_derived_segment_reads_back_with_its_vectors() {
    for name in ["knn-l2", "knn-cosine", "knn-innerproduct"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/segments/files/{name}"));
        let dir = FsDirectory::open(&path);
        let reader = DirectoryReader::open(&dir).expect("open");
        let segment = &reader.segment_readers()[0];
        assert!(segment.derived_source().is_some(), "{name}: derived fields");
        for (doc, (raw, rebuilt)) in STOCK_RAW.iter().zip(REBUILT).enumerate() {
            let (got_raw, got) = sources(segment, i32::try_from(doc).expect("doc"));
            assert_eq!(&got_raw, raw, "{name}: raw {doc}");
            assert_eq!(got, rebuilt, "{name}: read {doc}");
        }
    }
}

/// A shard over the knn engine indexing the fixture's documents, in order.
fn shard_of<'d>(fixture: &Value, dir: &'d ByteBuffersDirectory) -> Shard<'d> {
    let mapping = Mapping::parse_with(
        fixture["mappings"].to_string().as_bytes(),
        ParseOptions { knn: Some(true) },
    )
    .expect("mapping");
    let (writer, derived) = open_writer(dir, &mapping).expect("writer");
    assert!(derived);
    let mut shard = Shard::new(dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived);
    for (n, d) in fixture["documents"]
        .as_array()
        .expect("documents")
        .iter()
        .enumerate()
    {
        let id = d["id"].as_str().expect("id");
        // What the plugin read back, in the order the document gave its keys.
        let source = recorded_source(fixture, n);
        shard
            .index(id, source.as_bytes(), false, &Expect::default())
            .unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }
    shard
}

// AC-2, AC-3
#[test]
fn a_knn_shard_stores_what_the_stock_plugin_stores_and_reads_vectors_back() {
    let fixture = fixture("knn-l2");
    let dir = ByteBuffersDirectory::new();
    shard_of(&fixture, &dir).refresh().expect("refresh");
    let reader = DirectoryReader::open(&dir).expect("open");
    let segments = reader.segment_readers();
    assert_eq!(segments.len(), 1);
    let segment = &segments[0];
    let infos = lucene_index::segment_infos::read_latest(&dir).expect("commit");
    assert_eq!(infos.segments[0].codec_name, KNN_CODEC);
    assert!(segment.derived_source().is_some());
    for (doc, (raw, rebuilt)) in STOCK_RAW.iter().zip(REBUILT).enumerate() {
        let (got_raw, got) = sources(segment, i32::try_from(doc).expect("doc"));
        assert_eq!(&got_raw, raw, "raw {doc}");
        assert_eq!(got, rebuilt, "read {doc}");
    }
}

// AC-3
#[test]
fn an_update_and_a_realtime_get_read_the_new_vector_back() {
    let fixture = fixture("knn-l2");
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard_of(&fixture, &dir);
    shard
        .index("1", br#"{"v":[9,8,7],"k":"c"}"#, false, &Expect::default())
        .expect("update");
    let got = shard.get("1", true).expect("get").expect("found");
    assert_eq!(
        String::from_utf8(got.source).expect("utf8"),
        r#"{"v":[9.0,8.0,7.0],"k":"c"}"#
    );
}

// AC-6
#[test]
fn an_index_without_a_vector_field_writes_the_codec_and_source_it_wrote_before() {
    let mapping = Mapping::parse(br#"{"properties":{"k":{"type":"keyword"}}}"#).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let (writer, derived) = open_writer(&dir, &mapping).expect("writer");
    assert!(!derived);
    let mut shard = Shard::new(&dir, writer, mapping, 1, Box::new(|| 0)).expect("shard");
    shard
        .index("1", br#"{"k":"a"}"#, false, &Expect::default())
        .expect("index");
    shard.refresh().expect("refresh");
    let infos = lucene_index::segment_infos::read_latest(&dir).expect("commit");
    assert_eq!(infos.segments[0].codec_name, "Lucene104");
    let reader = DirectoryReader::open(&dir).expect("open");
    assert!(reader.segment_readers()[0].derived_source().is_none());
}
