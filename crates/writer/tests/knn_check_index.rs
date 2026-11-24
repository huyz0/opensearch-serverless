//! The `knn-*` segment fixtures' documents, written by a shard over the knn
//! engine to a directory for Java to verify (run by hand, ignored).

#![allow(clippy::disallowed_methods)]

use std::path::Path;

use lucene_store::FsDirectory;
use oss_writer::index::knn::open_writer;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};
use serde_json::Value;

/// Writes each `knn-*` fixture's documents to a directory of its own under
/// `KNN_SEGMENT_DIR`, for Lucene's `CheckIndex` and `SegmentDump` (with the
/// k-NN plugin on the class path) to read:
///
/// ```text
/// KNN_SEGMENT_DIR=/tmp/knn cargo test -p oss-writer --test knn_check_index -- --ignored
/// java -cp <lucene-core and the plugin's jars> org.apache.lucene.index.CheckIndex /tmp/knn/knn-cosine
/// java -cp <the same, and fixtures/segments> SegmentDump /tmp/knn/knn-cosine
/// ```
///
/// `SegmentDump`'s output is the fixture's `segment` but for the segment
/// ids and the diagnostics.
#[test]
#[ignore = "writes to KNN_SEGMENT_DIR for a Java check"]
fn knn_segments_are_written_for_java_to_check() {
    let out = std::env::var("KNN_SEGMENT_DIR").expect("KNN_SEGMENT_DIR");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/segments");
    for name in ["knn-l2", "knn-cosine", "knn-innerproduct"] {
        let fixture: Value = serde_json::from_slice(
            &std::fs::read(fixtures.join(format!("{name}.json"))).expect("read"),
        )
        .expect("fixture json");
        let mapping = Mapping::parse_with(
            fixture["mappings"].to_string().as_bytes(),
            ParseOptions { knn: Some(true) },
        )
        .expect("mapping");
        let target = Path::new(&out).join(name);
        std::fs::create_dir_all(&target).expect("dir");
        let dir = FsDirectory::open(&target);
        let (writer, derived) = open_writer(&dir, &mapping).expect("writer");
        let mut shard = Shard::new(&dir, writer, mapping, 1, Box::new(|| 0))
            .expect("shard")
            .with_derived_source(derived);
        for (n, d) in fixture["documents"]
            .as_array()
            .expect("documents")
            .iter()
            .enumerate()
        {
            let id = d["id"].as_str().expect("id");
            let stored = fixture["segment"]["docs"][n]["stored"]
                .as_array()
                .expect("stored");
            let source = stored.iter().find(|f| f[0] == "_source").expect("_source")[2]
                .as_str()
                .expect("text");
            shard
                .index(id, source.as_bytes(), false, &Expect::default())
                .unwrap_or_else(|e| panic!("{id}: {e:?}"));
        }
        shard.refresh().expect("refresh");
    }
}
