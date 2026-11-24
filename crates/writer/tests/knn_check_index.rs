//! The `knn-*` segment fixtures' documents, written by the Rust writer to a
//! directory for the Java `CheckIndex` to verify (run by hand, ignored).

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use oss_writer::document;
use oss_writer::lucene::{Indexer, Meta};
use oss_writer::mapping::{Mapping, ParseOptions};
use serde_json::Value;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/segments");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    out.sort();
    out
}

fn recorded_source(fixture: &Value, doc: usize) -> String {
    let stored = fixture["segment"]["docs"][doc]["stored"]
        .as_array()
        .expect("stored");
    let source = stored.iter().find(|f| f[0] == "_source").expect("_source");
    source[2].as_str().expect("text").to_owned()
}

/// Writes each `knn-*` fixture's documents to a directory of its own under
/// `KNN_SEGMENT_DIR`, for Lucene's own `CheckIndex` to read:
///
/// ```text
/// KNN_SEGMENT_DIR=/tmp/knn cargo test -p oss-writer --test knn_check_index -- --ignored
/// java -cp lucene-core.jar org.apache.lucene.index.CheckIndex /tmp/knn/knn-cosine
/// ```
#[test]
#[ignore = "writes to KNN_SEGMENT_DIR for a Java check"]
fn knn_segments_are_written_for_java_to_check() {
    let out = std::env::var("KNN_SEGMENT_DIR").expect("KNN_SEGMENT_DIR");
    for path in fixtures().iter().filter(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("knn-"))
    }) {
        let fixture: Value =
            serde_json::from_slice(&std::fs::read(path).expect("read")).expect("fixture json");
        let mapping = Mapping::parse_with(
            fixture["mappings"].to_string().as_bytes(),
            ParseOptions { knn: Some(true) },
        )
        .expect("mapping");
        let target = Path::new(&out).join(path.file_stem().expect("stem"));
        std::fs::create_dir_all(&target).expect("dir");
        let dir = lucene_store::FsDirectory::open(&target);
        let mut indexer = Indexer::new();
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        for (seq_no, d) in fixture["documents"]
            .as_array()
            .expect("documents")
            .iter()
            .enumerate()
        {
            let id = d["id"].as_str().expect("id");
            let source = recorded_source(&fixture, seq_no);
            let parsed = document::parse(&mapping, id, source.as_bytes()).expect("parse");
            let meta = Meta {
                id,
                seq_no: i64::try_from(seq_no).expect("fits"),
                primary_term: 1,
                version: 1,
                source: source.as_bytes(),
            };
            let (doc, vectors) = indexer
                .document_with_vectors(&mut w, &mapping, &parsed, &meta, None)
                .expect("document");
            if vectors.is_empty() {
                w.add_explicit_documents(vec![doc]).expect("add");
            } else {
                w.add_explicit_documents_with_vectors(None, vec![doc], vec![vectors])
                    .expect("add");
            }
        }
        w.commit().expect("commit");
    }
}
