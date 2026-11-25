//! A k-NN shard merges its segments and keeps every vector.

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::knn::open_writer;
use oss_writer::index::{Expect, MergeGate, MergeGuard, MergeSettings, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

const MAPPING: &str = r#"{"properties":{"v":{"type":"knn_vector","dimension":3,"method":{"name":"hnsw","engine":"lucene","space_type":"l2","parameters":{"m":4,"ef_construction":20}}}}}"#;

struct Open;

impl MergeGate for Open {
    fn try_start(&self, _: u64) -> Option<Box<dyn MergeGuard>> {
        Some(Box::new(()))
    }
}

// docs/specs/writer-segment-merge AC-2
#[test]
fn a_merged_knn_shard_holds_every_vector_in_fewer_segments() {
    let mapping =
        Mapping::parse_with(MAPPING.as_bytes(), ParseOptions { knn: Some(true) }).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let (writer, derived) = open_writer(&dir, &mapping).expect("writer");
    let mut shard = Shard::new(&dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived)
        .with_merging(
            MergeSettings {
                segments_per_tier: 4,
                max_merge_at_once: 4,
                max_merged_segment_bytes: 64 << 20,
            },
            Arc::new(Open),
        );
    for i in 0..40u32 {
        let body = format!("{{\"v\":[{i},{},{}]}}", i + 1, i + 2);
        shard
            .index(&i.to_string(), body.as_bytes(), false, &Expect::default())
            .expect("indexed");
        shard.refresh().expect("refresh");
    }
    let reader = DirectoryReader::open(&dir).expect("open");
    let segments = reader.segment_readers();
    assert!(segments.len() < 20, "{} segments", segments.len());
    let mut firsts = Vec::new();
    for segment in segments {
        let number = segment
            .field_infos()
            .fields
            .iter()
            .find(|f| f.name == "v")
            .expect("field v")
            .number;
        let (flat, _graph) = segment
            .knn_vectors_for(number)
            .expect("vector files")
            .expect("a field with vectors");
        let values = flat.float_vector_values(number).expect("values");
        let all = values.all_vectors().expect("vectors");
        firsts.extend(all.chunks(3).map(|v| v[0] as u32));
    }
    firsts.sort_unstable();
    assert_eq!(firsts, (0..40).collect::<Vec<u32>>(), "every vector, once");
}
