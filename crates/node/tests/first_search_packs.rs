//! docs/specs/pack-first-search: the first open of 1,000 shards whose published
//! commits live in node packs, against the same shards placed as blobs.
//! Evidence run by hand: `cargo test -p oss-node --features writer --release
//! --test first_search_packs -- --ignored --nocapture`.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::emulator::faults::Latency;
use oss_blob::{BlobStore, Emulator, PooledStore, S3Op, Shared};
use oss_format::ShardRef;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::pack::{PackLedger, PackStore, PackWriter};
use oss_writer::publish::{PackPlacement, ShardPublisher};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};
const NODE: &str = "node-1";
const SHARDS: u32 = 1_000;
const DOCS_PER_SHARD: usize = 4;
/// Threads publishing, and threads opening: a node's open path is bounded.
const PUBLISH_THREADS: usize = 50;
const OPEN_THREADS: usize = 16;
const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

/// What the first open of every shard cost.
struct Opened {
    took: Duration,
    requests: u64,
    gets: u64,
    docs: u64,
}

fn latency(store: &Emulator, base: Duration) {
    for op in [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List] {
        store.set_latency(
            op,
            Latency {
                base,
                jitter: base / 3,
            },
        );
    }
}

/// `SHARDS` shards of one index published (into packs when `packed`), then
/// every one opened cold by a reader over the same store at the Java soak's
/// mean latency.
fn first_open(packed: bool) -> Opened {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let dynstore = || Arc::clone(&store) as Arc<dyn BlobStore>;
    store
        .create_register(
            "indices/logs",
            format!(
                r#"{{"name":"logs","uuid":"u1","number_of_shards":{SHARDS},"mapping":{},"settings":{{}}}}"#,
                serde_json::to_string(MAPPING).expect("json")
            )
            .as_bytes(),
        )
        .expect("descriptor");
    let writer = Arc::new(PackWriter::new(
        PackStore::new(dynstore(), "", NODE),
        Some(PackLedger::new(dynstore(), "", NODE).expect("ledger")),
        11,
        Duration::from_millis(50),
        64 << 20,
    ));
    let dirs: Vec<ByteBuffersDirectory> =
        (0..SHARDS).map(|_| ByteBuffersDirectory::new()).collect();
    let mut shards: Vec<_> = dirs
        .iter()
        .zip(0..SHARDS)
        .map(|(dir, i)| {
            let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
            w.enable_explicit_documents().expect("explicit");
            let mut shard = Shard::new(
                dir,
                w,
                Mapping::parse(MAPPING.as_bytes()).expect("mapping"),
                3,
                Box::new(|| 0),
            )
            .expect("shard");
            for d in 0..DOCS_PER_SHARD {
                shard
                    .index(&format!("d{d}"), br#"{"n":1}"#, false, &Expect::default())
                    .expect("indexed");
            }
            shard.refresh().expect("refresh");
            let shard_ref = ShardRef {
                index: "logs",
                uuid: "u1",
                shard: i,
            };
            let mut publisher = ShardPublisher::new(dynstore(), "", &shard_ref).expect("publisher");
            if packed {
                publisher.use_packs(Some(PackPlacement {
                    writer: Arc::clone(&writer),
                    index_name: "logs".to_owned(),
                    index_uuid: "u1".to_owned(),
                    shard: i,
                    max_blob_bytes: 8 << 20,
                }));
            }
            (shard, publisher)
        })
        .collect();
    let mut work: Vec<_> = shards
        .iter_mut()
        .zip(&dirs)
        .map(|((shard, publisher), dir)| (publisher, dir, shard.committed_files().expect("files")))
        .collect();
    let chunk = work.len().div_ceil(PUBLISH_THREADS);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        while !work.is_empty() {
            let rest = work.split_off(chunk.min(work.len()));
            let mine = std::mem::replace(&mut work, rest);
            handles.push(scope.spawn(move || {
                for (publisher, dir, files) in mine {
                    publisher
                        .publish(dir, &files, 3, NODE, 0, 1)
                        .expect("published");
                }
            }));
        }
        for h in handles {
            h.join().expect("thread");
        }
    });

    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 64).expect("fits");
    let reader = Arc::new(PooledStore::new(Shared::new(dynstore()), buffers));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    latency(&store, Duration::from_millis(20));
    let (before, gets) = (
        store.meter().total(),
        store.meter().count(S3Op::Get) + store.meter().count(S3Op::GetRange),
    );
    let lists = store.meter().count(S3Op::List);
    let next = std::sync::atomic::AtomicU32::new(0);
    let docs = std::sync::atomic::AtomicU64::new(0);
    let start = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..OPEN_THREADS {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= SHARDS {
                        break;
                    }
                    let open = oss_reader::open_shard(
                        Arc::clone(&reader),
                        Arc::clone(&cache),
                        "",
                        "logs",
                        i,
                    )
                    .expect("opens");
                    docs.fetch_add(
                        u64::from(open.reader.num_docs().unsigned_abs()),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
            });
        }
    });
    let took = start.elapsed();
    assert_eq!(
        store.meter().count(S3Op::List),
        lists,
        "an open lists nothing"
    );
    Opened {
        took,
        requests: store.meter().total() - before,
        gets: store.meter().count(S3Op::Get) + store.meter().count(S3Op::GetRange) - gets,
        docs: docs.into_inner(),
    }
}

// docs/specs/pack-first-search AC-1, AC-2, AC-3
#[test]
#[ignore = "evidence: run by hand with --release --nocapture"]
fn the_first_open_of_a_thousand_packed_shards_costs_what_the_blob_open_does() {
    let blob = first_open(false);
    let packed = first_open(true);
    eprintln!(
        "blob {:?}, {} requests ({} reads); packed {:?}, {} requests ({} reads)",
        blob.took, blob.requests, blob.gets, packed.took, packed.requests, packed.gets
    );
    let all = u64::from(SHARDS) * DOCS_PER_SHARD as u64;
    assert_eq!(blob.docs, all, "blob shards lost documents");
    assert_eq!(packed.docs, all, "packed shards lost documents");
    assert!(
        packed.took.as_millis() * 100 <= blob.took.as_millis() * 105,
        "packed opened in {:?} against blob {:?}",
        packed.took,
        blob.took
    );
    assert!(
        packed.requests <= blob.requests,
        "packed made {} requests against blob {}",
        packed.requests,
        blob.requests
    );
}
