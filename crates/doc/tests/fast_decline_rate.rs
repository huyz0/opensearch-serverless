//! How often the plan declines a document on a fully dynamic mapping: the shapes a real index
//! sees (flat logs, nested objects, tag arrays, optional fields). Release only
//! (`cargo test --release -p oss-doc --test fast_decline_rate -- --ignored --nocapture`).

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

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

fn source(i: usize) -> String {
    match i % 4 {
        0 => format!(
            r#"{{"@timestamp":"2026-11-20T08:{:02}:{:02}.000Z","level":"info","host":"web-{}","status":{},"bytes":{},"message":"GET /items/{} ok"}}"#,
            (i / 60) % 60,
            i % 60,
            i % 50,
            200 + i % 5,
            i * 13,
            i
        ),
        1 => format!(
            r#"{{"user":{{"id":{i},"name":"user{}","geo":{{"country":"US"}}}},"tags":["a","b{}"],"score":{}.5,"ok":true}}"#,
            i % 100,
            i % 7,
            i % 10
        ),
        2 => format!(
            r#"{{"order":{{"id":"o{i}","total":{}.25,"items":[{{"sku":"s{}","qty":{}}},{{"sku":"s{}","qty":1}}]}}}}"#,
            i % 500,
            i % 30,
            i % 4,
            i % 31
        ),
        _ => format!(
            r#"{{"@timestamp":"2026-11-20T09:00:{:02}.000Z","level":"warn","host":"web-{}","status":{},"extra":{{"k{}":"v"}}{}}}"#,
            i % 60,
            i % 50,
            400 + i % 5,
            i % 20,
            if i % 8 == 3 { r#","note":null"# } else { "" }
        ),
    }
}

#[test]
#[ignore = "a release-mode measurement"]
fn real_shapes_on_a_dynamic_mapping_rarely_decline() {
    let dir = ByteBuffersDirectory::new();
    let mut writer = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    writer.enable_explicit_documents().expect("explicit");
    let mut indexer = Indexer::new();
    let mut mapping = Mapping::parse(br#"{"properties":{}}"#).expect("mapping");
    let mut fast = FastIngest::new();
    let (mut taken, mut declined) = (0_usize, 0_usize);
    let n = 20_000;
    let mut late = 0_usize;
    for i in 0..n {
        let src = source(i).into_bytes();
        let id = i.to_string();
        let key = uid(&id).expect("uid");
        if let Some(f) = fast.prepare(&indexer, &mapping, &src, &key) {
            std::hint::black_box(f.finish(0, 1, 1));
            taken += 1;
            continue;
        }
        declined += 1;
        if i >= n / 2 {
            late += 1;
        }
        let parsed = document::parse(&mapping, &id, &src).expect("parse");
        if let Some(g) = &parsed.grown {
            mapping = g.clone();
            fast.invalidate();
        }
        let meta = Meta {
            id: &id,
            seq_no: 0,
            primary_term: 1,
            version: 1,
            source: &src,
        };
        std::hint::black_box(
            indexer
                .document(&mut writer, &mapping, &parsed, &meta)
                .expect("document"),
        );
    }
    eprintln!(
        "{taken} of {n} read by the plan, {declined} declined ({late} in the second half), {} compiles",
        fast.compiles()
    );
}
