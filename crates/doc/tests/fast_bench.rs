//! The fast path against the complete path on log-shaped documents: building each document's
//! Lucene fields. Release only (`cargo test --release -p oss-doc --test fast_bench -- --ignored
//! --nocapture`); numbers on an emulator-free in-memory directory are provisional.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::time::Instant;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::fast::FastIngest;
use oss_doc::lucene::{Indexer, Meta, uid};
use oss_doc::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const MAPPING: &str = r#"{"properties":{
  "@timestamp":{"type":"date"},"level":{"type":"keyword"},"host":{"type":"keyword"},
  "status":{"type":"integer"},"bytes":{"type":"long"},"message":{"type":"text"}}}"#;

fn source(i: usize) -> Vec<u8> {
    format!(
        r#"{{"@timestamp":"2026-11-20T08:{:02}:{:02}.000Z","level":"info","host":"web-{}","status":{},"bytes":{},"message":"GET /api/v1/items/{} returned ok after {} ms from client agent mozilla"}}"#,
        (i / 60) % 60,
        i % 60,
        i % 50,
        200 + (i % 5),
        i * 13,
        i,
        i % 900
    )
    .into_bytes()
}

#[test]
#[ignore = "a release-mode measurement"]
fn the_plan_builds_log_documents_at_least_twice_as_fast() {
    let docs: Vec<Vec<u8>> = (0..50_000).map(source).collect();
    let mapping = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let mut writer = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    writer.enable_explicit_documents().expect("explicit");
    let mut indexer = Indexer::new();
    let mut fast = FastIngest::new();
    let (mut slow_best, mut fast_best) = (f64::MAX, f64::MAX);
    for _ in 0..3 {
        let t = Instant::now();
        for (i, d) in docs.iter().enumerate() {
            let id = i.to_string();
            let parsed = document::parse(&mapping, &id, d).expect("parse");
            let meta = Meta {
                id: &id,
                seq_no: 0,
                primary_term: 1,
                version: 1,
                source: d,
            };
            let doc = indexer
                .document(&mut writer, &mapping, &parsed, &meta)
                .expect("document");
            std::hint::black_box(doc);
        }
        slow_best = slow_best.min(t.elapsed().as_secs_f64());
        let t = Instant::now();
        let mut taken = 0_usize;
        for (i, d) in docs.iter().enumerate() {
            let key = uid(&i.to_string()).expect("uid");
            let doc = fast
                .prepare(&indexer, &mapping, d, &key)
                .expect("the plan reads a log document")
                .finish(0, 1, 1);
            taken += doc.len();
            std::hint::black_box(doc);
        }
        assert_eq!(taken, docs.len());
        fast_best = fast_best.min(t.elapsed().as_secs_f64());
    }
    let ratio = slow_best / fast_best;
    eprintln!("complete {slow_best:.3}s, plan {fast_best:.3}s: {ratio:.1}x");
    assert!(ratio >= 2.0, "{ratio:.1}x");
}
