//! One large source (a text field of several MB beside mapped fields) built by the plan and by
//! the complete path: time and the process's peak memory. Release only; run once with
//! `LARGE=fast` and once with `LARGE=slow` (peak memory is per process):
//! `LARGE=fast cargo test --release -p oss-doc --test fast_large_source -- --ignored --nocapture`.

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

// /proc/self/status is a few hundred bytes.
#[allow(clippy::disallowed_methods)]
fn peak_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

#[test]
#[ignore = "a release-mode measurement"]
fn a_large_source_costs_the_plan_no_more_memory_than_the_complete_path() {
    let which = std::env::var("LARGE").unwrap_or_else(|_| "fast".into());
    let mb: usize = std::env::var("LARGE_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let mut body = String::new();
    let mut i = 0_usize;
    while body.len() < mb * 1_000_000 {
        body.push_str("word");
        body.push_str(&(i % 5000).to_string());
        body.push(' ');
        i += 1;
    }
    let src = format!(r#"{{"host":"h1","status":200,"message":"{body}"}}"#).into_bytes();
    let mapping = Mapping::parse(
        br#"{"properties":{"host":{"type":"keyword"},"status":{"type":"integer"},"message":{"type":"text"}}}"#,
    )
    .expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let mut writer = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    writer.enable_explicit_documents().expect("explicit");
    let mut indexer = Indexer::new();
    let mut fast = FastIngest::new();
    let (before, t) = (peak_kb(), Instant::now());
    let key = uid("1").expect("uid");
    let build = |fast: &mut FastIngest, indexer: &mut Indexer, w: &mut IndexWriter<'_>| {
        if which == "fast"
            && let Some(f) = fast.prepare(indexer, &mapping, &src, &key)
        {
            return f.finish(0, 1, 1);
        }
        let parsed = document::parse(&mapping, "1", &src).expect("parse");
        let meta = Meta {
            id: "1",
            seq_no: 0,
            primary_term: 1,
            version: 1,
            source: &src,
        };
        vec![indexer.document(w, &mapping, &parsed, &meta).expect("doc")]
    };
    // The first document numbers the fields on both paths; the second is the measured shape.
    drop(build(&mut fast, &mut indexer, &mut writer));
    let docs = build(&mut fast, &mut indexer, &mut writer);
    eprintln!(
        "{which}: {mb} MB source, {:.3}s for two, peak {} MB (was {} MB before), {} docs",
        t.elapsed().as_secs_f64(),
        peak_kb() / 1000,
        before / 1000,
        docs.len()
    );
}
