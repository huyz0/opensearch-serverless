//! docs/specs/knn-vector-index: a shard indexes `knn_vector` documents, an
//! update and a document without a vector, and the committed segment holds
//! each vector bit for bit, in document order.

#![allow(clippy::disallowed_methods)]

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

const MAPPING: &str = r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"name":"hnsw","engine":"lucene","space_type":"cosinesimil","parameters":{"m":4,"ef_construction":20}}},"k":{"type":"keyword"}}}"#;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn index(shard: &mut Shard<'_>, id: &str, source: &str) {
    shard
        .index(id, source.as_bytes(), false, &Expect::default())
        .unwrap_or_else(|e| panic!("{id}: {e:?}"));
}

#[test]
fn a_shard_writes_each_vector_it_indexes_and_an_update_adds_the_new_one() {
    let mapping =
        Mapping::parse_with(MAPPING.as_bytes(), ParseOptions { knn: Some(true) }).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
        index(&mut shard, "1", r#"{"v":[1,2,3],"k":"a"}"#);
        index(&mut shard, "2", r#"{"v":[0.5,-1.25,300.0],"k":"b"}"#);
        index(&mut shard, "3", r#"{"k":"no vector"}"#);
        // An update takes the soft-delete path, with a vector.
        index(&mut shard, "1", r#"{"v":[9,8,7],"k":"c"}"#);
        shard.refresh().expect("refresh");
    }
    let reader = DirectoryReader::open(&dir).expect("open");
    let segments = reader.segment_readers();
    assert_eq!(segments.len(), 1);
    let number = segments[0]
        .field_infos()
        .fields
        .iter()
        .find(|f| f.name == "v")
        .expect("field v")
        .number;
    let (flat, _graph) = segments[0]
        .knn_vectors_for(number)
        .expect("vector files")
        .expect("a field with vectors");
    let values = flat.float_vector_values(number).expect("values");
    assert_eq!(values.size(), 3);
    let docs: Vec<i32> = (0..3)
        .map(|ord| values.ord_to_doc(ord).expect("doc"))
        .collect();
    assert_eq!(docs, vec![0, 1, 3], "the documents with a vector, in order");
    let all = values.all_vectors().expect("vectors");
    assert_eq!(
        all.as_ref(),
        &[1.0, 2.0, 3.0, 0.5, -1.25, 300.0, 9.0, 8.0, 7.0]
    );
}

// docs/adr/0038: a vector counts in the indexing buffer, which the node
// bounds and flushes by, so vector documents cannot grow past it unseen.
// A thousand ones is two thousand bytes of `_source` but four thousand bytes
// of vector, so a buffer that holds the source alone stays under the bound.
#[test]
fn vectors_are_charged_to_the_indexing_buffer() {
    const DIMENSION: usize = 1000;
    const DOCS: usize = 50;
    let mapping = format!(
        r#"{{"properties":{{"v":{{"type":"knn_vector","dimension":{DIMENSION},"method":{{"name":"hnsw","engine":"lucene","space_type":"l2"}}}}}}}}"#
    );
    let mapping =
        Mapping::parse_with(mapping.as_bytes(), ParseOptions { knn: Some(true) }).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
    let ones = vec!["1"; DIMENSION].join(",");
    for n in 0..DOCS {
        index(
            &mut shard,
            &format!("d{n}"),
            &format!(r#"{{"v":[{ones}]}}"#),
        );
    }
    let buffered = shard.indexing_buffer_bytes();
    assert!(
        buffered >= DOCS * DIMENSION * 4,
        "{DOCS} vectors of {DIMENSION} floats are {} bytes, the buffer holds {buffered}",
        DOCS * DIMENSION * 4
    );
}
