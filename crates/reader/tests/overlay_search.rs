//! A search over a commit and its tail overlay (docs/specs/reader-overlay-search).

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, S3Op, SliceSource};
use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::{MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_reader::overlay::{Overlay, Ready, find_copies};
use oss_reader::response::write_response;
use oss_reader::search::search;
use oss_reader::{OpenShard, open_shard};
use oss_testkit::index::{Spec, blobs, mapping};

const INDEX: &str = "ov";

fn open() -> (Arc<PooledStore<Emulator>>, OpenShard<Emulator>, Mapping) {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(Emulator::new(64 << 20, 1), buffers));
    let spec = Spec::new(INDEX, 40).segments_of(10);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let shard = open_shard(Arc::clone(&store), cache, "", INDEX, 0).expect("open");
    let doc_mapping = Mapping::parse(mapping(&spec).as_bytes()).expect("mapping");
    (store, shard, doc_mapping)
}

fn record(id: &str, tag: &str, n: i64, seq: i64) -> WalRecord<'static> {
    WalRecord::index(
        id.to_owned(),
        format!(r#"{{"tag":"{tag}","n":{n}}}"#),
        seq,
        1,
        seq + 1,
    )
}

/// The overlay of: `doc.1` rewritten as tag `zz`, `doc.2` deleted, `doc.new` added.
fn tail(mapping: &Mapping) -> Overlay {
    let records = [
        record("doc.1", "zz", 500, 1001),
        WalRecord::deletion("doc.2".to_owned(), 1002, 1, 1003),
        record("doc.new", "zz", 1000, 1003),
    ];
    Overlay::build(mapping, &records, 0, 4 << 20).expect("builds")
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> String {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokenizer = source::tokenizer();
    tokenizer.feed(body.as_bytes(), &mut rec).expect("feeds");
    tokenizer.finish(&mut rec).expect("ends");
    let source = SearchSource::parse(&rec).expect("parses");
    let hits = search(shard, INDEX, &source, &Analyzer::standard(None)).expect("searches");
    let mut out = Vec::new();
    write_response(&mut out, shard, INDEX, "node", &hits, 0, &mut |_| Ok(())).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn total(response: &str) -> u64 {
    let at = response.find(r#""total":{"value":"#).expect("a total") + r#""total":{"value":"#.len();
    response[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|n| n.parse().ok())
        .expect("a number")
}

fn tag_count(shard: &OpenShard<Emulator>, tag: &str) -> u64 {
    total(&run(
        shard,
        &format!(r#"{{"size":0,"track_total_hits":true,"query":{{"term":{{"tag":"{tag}"}}}}}}"#),
    ))
}

#[test]
fn a_search_finds_the_overlays_documents_and_not_the_copies_they_replace() {
    let (store, shard, mapping) = open();
    let overlay = tail(&mapping);
    let copies = find_copies(&shard.reader, overlay.ids()).expect("copies");
    let before = store.store().meter().count(S3Op::GetRange);
    let view = shard.with_overlay(&overlay, &copies).expect("view");
    assert_eq!(store.store().meter().count(S3Op::GetRange), before);
    // doc.1 had tag t1 with doc.17 and doc.33; doc.2 had t2 with 18 and 34.
    assert_eq!(tag_count(&view, "t1"), 2);
    assert_eq!(tag_count(&view, "t2"), 2);
    assert_eq!(tag_count(&view, "zz"), 2);
    let hits = run(
        &view,
        r#"{"size":10,"query":{"term":{"tag":"zz"}},"sort":[{"n":"desc"}]}"#,
    );
    assert!(hits.contains(r#""_id":"doc.new""#), "{hits}");
    assert!(hits.contains(r#""_source":{"tag":"zz","n":500}"#), "{hits}");
    // Warm now: a repeat search over the view reads nothing from the store.
    let warm = store.store().meter().count(S3Op::GetRange);
    tag_count(&view, "zz");
    assert_eq!(store.store().meter().count(S3Op::GetRange), warm);
}

#[test]
fn a_sort_and_an_aggregation_see_the_overlay() {
    let (_store, shard, mapping) = open();
    let overlay = tail(&mapping);
    let copies = find_copies(&shard.reader, overlay.ids()).expect("copies");
    let view = shard.with_overlay(&overlay, &copies).expect("view");
    let top = run(
        &view,
        r#"{"size":2,"sort":[{"n":"desc"}],"query":{"match_all":{}}}"#,
    );
    let new = top
        .find(r#""_id":"doc.new""#)
        .expect("the new document first");
    let next = top
        .find(r#""_id":"doc.1""#)
        .expect("then the rewritten one");
    assert!(new < next, "{top}");
    let max = run(&view, r#"{"size":0,"aggs":{"m":{"max":{"field":"n"}}}}"#);
    assert!(max.contains(r#""value":1000"#), "{max}");
}

#[test]
fn an_overlay_with_no_document_answers_as_the_commit() {
    let (_store, shard, mapping) = open();
    let overlay = Overlay::build(&mapping, &[], 0, 4 << 20).expect("builds");
    let copies = find_copies(&shard.reader, overlay.ids()).expect("copies");
    let view = shard.with_overlay(&overlay, &copies).expect("view");
    let body = r#"{"size":50,"sort":[{"n":"asc"}],"query":{"match_all":{}}}"#;
    assert_eq!(run(&view, body), run(&shard, body));
}

#[test]
fn making_a_view_leaves_the_commit_as_it_was() {
    let (_store, shard, mapping) = open();
    let before = tag_count(&shard, "t1");
    let overlay = tail(&mapping);
    let copies = find_copies(&shard.reader, overlay.ids()).expect("copies");
    let _view = shard.with_overlay(&overlay, &copies).expect("view");
    assert_eq!(tag_count(&shard, "t1"), before);
    assert_eq!(tag_count(&shard, "zz"), 0);
    assert_eq!(shard.reader.max_doc(), 40);
}

#[test]
fn a_ready_counts_the_views_it_makes_and_shows_the_same_answers() {
    let (_store, shard, mapping) = open();
    let overlay = tail(&mapping);
    let copies = find_copies(&shard.reader, overlay.ids()).expect("copies");
    let ready = Ready::new(overlay, copies);
    assert_eq!(ready.views_made(), 0);
    assert_eq!(ready.overlay().ids().len(), 3);
    let first = ready.view(&shard).expect("view");
    assert_eq!(ready.views_made(), 1);
    let second = ready.view(&shard).expect("view");
    assert_eq!(ready.views_made(), 2);
    assert_eq!(tag_count(&first, "zz"), 2);
    assert_eq!(tag_count(&second, "zz"), 2);
}
