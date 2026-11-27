//! A writer holding 1,000 shards publishing into node packs: the requests a
//! publish makes and the heap the whole set holds. Its own binary, because
//! the counting allocator is process-wide.

use std::sync::Arc;
use std::time::Duration;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::ShardRef;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::pack::{PackLedger, PackStore, PackWriter};
use oss_writer::publish::{PackPlacement, ShardPublisher};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};
const NODE: &str = "node-1";
const SHARDS: usize = 1_000;
const ROUNDS: usize = 3;
const THREADS: usize = 50;
/// Requests per publish of the blob mode (writer soak).
const BLOB_MODE_REQUESTS_PER_PUBLISH: f64 = 2.0;
/// Publishes that land in one pack, at the least, on average.
const MIN_PUBLISHES_PER_PACK: u64 = 4;
/// Heap the open shards, directories and pack writer may hold; provisional.
const HEAP_BOUND: usize = 128 << 20;

#[test]
fn a_thousand_shards_publish_into_shared_packs() {
    let dirs: Vec<ByteBuffersDirectory> =
        (0..SHARDS).map(|_| ByteBuffersDirectory::new()).collect();
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let dynstore = || Arc::clone(&store) as Arc<dyn BlobStore>;
    let writer = Arc::new(PackWriter::new(
        PackStore::new(dynstore(), "", NODE),
        Some(PackLedger::new(dynstore(), "", NODE).expect("ledger")),
        11,
        Duration::from_millis(50),
        64 << 20,
    ));
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
            let mut publisher = ShardPublisher::new(dynstore(), "", &shard_ref).expect("publisher");
            publisher.use_packs(Some(PackPlacement {
                writer: Arc::clone(&writer),
                index_name: "logs".to_owned(),
                index_uuid: uuid.clone(),
                shard: 0,
                max_blob_bytes: 8 << 20,
            }));
            (shard, publisher)
        })
        .collect();

    let mut publishes = 0u64;
    for round in 0..ROUNDS {
        let ordinal = i64::try_from(round).expect("small");
        let work: Vec<_> = shards
            .iter_mut()
            .zip(&dirs)
            .map(|((shard, publisher), dir)| {
                shard
                    .index(
                        &format!("d{round}"),
                        br#"{"n":1}"#,
                        false,
                        &Expect::default(),
                    )
                    .expect("indexed");
                shard.refresh().expect("refresh");
                (publisher, dir, shard.committed_files().expect("files"))
            })
            .collect();
        let chunk = work.len().div_ceil(THREADS);
        let mut work = work;
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            while !work.is_empty() {
                let rest = work.split_off(chunk.min(work.len()));
                let mine = std::mem::replace(&mut work, rest);
                handles.push(scope.spawn(move || {
                    for (publisher, dir, files) in mine {
                        publisher
                            .publish(dir, &files, 3, NODE, ordinal, 1)
                            .expect("published");
                    }
                }));
            }
            for h in handles {
                h.join().expect("thread");
            }
        });
        publishes += SHARDS as u64;
    }
    let peak = window.peak_bytes();
    let puts = store.meter().count(S3Op::Put);
    let lists = store.meter().count(S3Op::List);
    let packs = writer.next_ordinal();
    // Every PUT is a swap, a pack or the one ledger create.
    assert_eq!(puts, publishes + packs + 1, "swaps, packs, one ledger");
    println!(
        "pack soak: {publishes} publishes, {puts} PUT ({packs} packs), {lists} LIST, peak heap {peak} B"
    );
    // Counts as f64 only to compare with the blob-mode ratio.
    #[allow(clippy::cast_precision_loss)]
    let per = (puts + lists) as f64 / publishes as f64;
    assert!(
        per < BLOB_MODE_REQUESTS_PER_PUBLISH,
        "{per} requests a publish"
    );
    assert_eq!(lists, 0, "a publish lists nothing");
    assert!(packs >= 1, "packs were written");
    assert!(
        packs * MIN_PUBLISHES_PER_PACK <= publishes,
        "{packs} packs for {publishes} publishes"
    );
    assert!(peak < HEAP_BOUND, "peak heap {peak} B");
}
