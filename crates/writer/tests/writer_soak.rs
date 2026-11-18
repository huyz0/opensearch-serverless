//! A writer holding 1,000 shards, each taking a document and publishing a
//! segment every round: the requests a publish makes and the heap the whole
//! set holds. Its own binary, because the counting allocator is process-wide.

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::ShardRef;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const SHARDS: usize = 1_000;
const ROUNDS: usize = 3;
/// Publish requests the Java writer makes for a refresh: the segment blob,
/// the register swap, and a ninth of the time a rewritten commit.
const JAVA_REQUESTS_PER_PUBLISH: f64 = 2.09;
/// Heap the 1,000 open shards and their directories may hold: 85.7 MB
/// measured, a half more allowed; provisional (emulator, one machine).
const HEAP_BOUND: usize = 128 << 20;

#[test]
fn a_thousand_shards_publish_with_the_requests_of_one_blob_and_one_swap_each() {
    let dirs: Vec<ByteBuffersDirectory> =
        (0..SHARDS).map(|_| ByteBuffersDirectory::new()).collect();
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let mapping = br#"{"properties":{"n":{"type":"integer"}}}"#;
    let window = PeakWindow::start(&ALLOC);
    let mut shards: Vec<(Shard<'_>, ShardPublisher)> = dirs
        .iter()
        .enumerate()
        .map(|(i, dir)| {
            let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
            w.enable_explicit_documents().expect("explicit");
            let shard = Shard::new(
                dir,
                w,
                Mapping::parse(mapping).expect("mapping"),
                3,
                Box::new(|| 0),
            )
            .expect("shard");
            let uuid = format!("u{i}");
            let shard_ref = ShardRef {
                index: "logs",
                uuid: &uuid,
                shard: 0,
            };
            let publisher =
                ShardPublisher::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", &shard_ref)
                    .expect("publisher");
            (shard, publisher)
        })
        .collect();

    let mut publishes = 0u64;
    for round in 0..ROUNDS {
        for (i, (shard, publisher)) in shards.iter_mut().enumerate() {
            shard
                .index(
                    &format!("d{round}"),
                    br#"{"n":1}"#,
                    false,
                    &Expect::default(),
                )
                .expect("indexed");
            shard.refresh().expect("refresh");
            let files = shard.committed_files().expect("files");
            let ordinal = i64::try_from(round).expect("small");
            publisher
                .publish(&dirs[i], &files, 3, "node-1", ordinal, 1)
                .expect("published");
            publishes += 1;
        }
    }
    let peak = window.peak_bytes();
    let puts = store.meter().count(S3Op::Put);
    let lists = store.meter().count(S3Op::List);
    println!("soak: {publishes} publishes, {puts} PUT, {lists} LIST, peak heap {peak} B");
    // Counts as f64 only to compare with the Java ratio.
    #[allow(clippy::cast_precision_loss)]
    let per = (puts + lists) as f64 / publishes as f64;
    assert!(per <= JAVA_REQUESTS_PER_PUBLISH, "{per} requests a publish");
    assert_eq!(puts, publishes * 2, "one blob and one swap each");
    assert_eq!(lists, 0, "a publish lists nothing");
    assert!(peak < HEAP_BOUND, "peak heap {peak} B");
}
