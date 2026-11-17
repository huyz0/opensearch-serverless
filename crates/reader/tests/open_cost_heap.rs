//! The open-cost estimate against the heap an open index retains, measured
//! by the counting allocator after the open and a mix of searches (a term
//! query, sorts on a long and a keyword, a match query on a text field, and
//! hits fetched), so what the engine loads on first use and keeps is
//! counted, over indexes of many shapes the engine built: never below it,
//! and at most eight times it or 128 KiB, whichever is larger.
//! Its own binary, because the counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::{MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_reader::open_cost::open_bytes;
use oss_reader::response::write_response;
use oss_reader::search::search;
use oss_reader::{OpenShard, ShardFiles, open_located};
use oss_testkit::index::{Spec, blobs};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

fn shapes() -> Vec<Spec> {
    vec![
        Spec::new("tiny", 10),
        Spec::new("small", 1_000),
        Spec::new("one-page-padded", 5_000).padded(1_000),
        Spec::new("compound", 20_000),
        Spec::new("large-compound", 200_000),
        Spec::new("separate", 20_000).separate_files(),
        Spec::new("segments", 20_000).segments_of(1_000),
        Spec::new("segments-separate", 20_000)
            .segments_of(1_000)
            .separate_files(),
        Spec::new("many-segments", 50_000).segments_of(250),
        Spec::new("keywords", 5_000).with_keywords(100),
        Spec::new("keywords-segments", 5_000)
            .segments_of(250)
            .with_keywords(100)
            .separate_files(),
        Spec::new("texts", 5_000).with_texts(100),
        Spec::new("texts-segments", 5_000)
            .segments_of(250)
            .with_texts(100)
            .separate_files(),
        Spec::new("mixed-small-segments", 2_000)
            .segments_of(100)
            .with_keywords(15)
            .with_texts(15),
    ]
}

/// The searches run on each index before its heap is read.
fn searches(spec: &Spec) -> Vec<String> {
    let mut out = vec![
        r#"{"size":10,"query":{"term":{"tag":"t3"}}}"#.to_owned(),
        r#"{"size":10,"sort":[{"n":"desc"}]}"#.to_owned(),
        r#"{"size":10,"sort":[{"tag":"asc"}]}"#.to_owned(),
    ];
    if spec.keywords > 0 {
        out.push(r#"{"size":10,"sort":[{"k0":"asc"}]}"#.to_owned());
    }
    if spec.texts > 0 {
        out.push(r#"{"size":10,"query":{"match":{"x0":"w1 u2"}}}"#.to_owned());
    }
    out
}

fn run(shard: &OpenShard<Emulator>, index: &str, body: &str) {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokenizer = source::tokenizer();
    tokenizer.feed(body.as_bytes(), &mut rec).expect("feeds");
    tokenizer.finish(&mut rec).expect("ends");
    let source = SearchSource::parse(&rec).expect("parses");
    let hits = search(shard, index, &source, &Analyzer::standard(None)).expect("searches");
    assert!(!hits.hits.is_empty(), "{index}: {body} found nothing");
    let mut out = Vec::new();
    write_response(&mut out, shard, index, "node", &hits, 0, &mut |_| Ok(())).expect("writes");
}

#[test]
fn the_estimate_bounds_what_an_open_retains() {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(Emulator::new(1 << 30, 1), buffers));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    // The process's first open starts the engine's pager and other
    // once-only state, which no index holds.
    let mut all = vec![Spec::new("warm-up", 10)];
    all.extend(shapes());
    let mut report = Vec::new();
    for (i, spec) in all.iter().enumerate() {
        for (key, bytes) in blobs("", spec) {
            store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
        }
        let (files, meta) = ShardFiles::open_with_meta(&store, "", &spec.name, 0).expect("locate");
        let estimate = open_bytes(&files, &meta, &cache);
        let window = PeakWindow::start(&ALLOC);
        let shard = open_located(
            Arc::clone(&store),
            Arc::clone(&cache),
            "",
            &spec.name,
            0,
            files,
            meta,
        )
        .expect("open");
        for body in searches(spec) {
            run(&shard, &spec.name, &body);
        }
        let retained = window.live_bytes();
        assert_eq!(
            usize::try_from(shard.reader.max_doc()).expect("fits"),
            spec.docs
        );
        if i > 0 {
            report.push((spec.name.clone(), retained, estimate));
        }
        drop(shard);
    }
    for (name, retained, estimate) in &report {
        println!("{name}: retained {retained} B, estimate {estimate} B");
    }
    for (name, retained, estimate) in &report {
        assert!(
            estimate >= retained,
            "{name}: estimate {estimate} under the {retained} B retained: {report:?}"
        );
        assert!(
            *estimate <= (8 * retained).max(128 << 10),
            "{name}: estimate {estimate} over eight times the {retained} B retained: {report:?}"
        );
    }
}
