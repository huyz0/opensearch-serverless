//! Lazy publish against the emulator: shards written every window publish
//! about once an age, at the requests of one blob and one swap each.

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::ShardRef;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;
use oss_writer::publish::lazy::{LazyPublish, Policy, PublishAge};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const SHARDS: usize = 20;
const WINDOW_MILLIS: u64 = 250;
const SECONDS: u64 = 10;

/// Runs `SECONDS` of one write per shard per window under `max_age`, and
/// returns (publishes, PUTs, LISTs).
fn run(max_age: u64) -> (u64, u64, u64) {
    let dirs: Vec<ByteBuffersDirectory> =
        (0..SHARDS).map(|_| ByteBuffersDirectory::new()).collect();
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let mapping = br#"{"properties":{"n":{"type":"integer"}}}"#;
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

    let mut lazy = LazyPublish::new(Policy::new(max_age, 0, WINDOW_MILLIS));
    let mut publishes = 0u64;
    let steps = SECONDS * 1_000 / WINDOW_MILLIS;
    for step in 0..steps {
        let now = step.saturating_mul(WINDOW_MILLIS);
        for (i, (shard, publisher)) in shards.iter_mut().enumerate() {
            shard
                .index(
                    &format!("d{step}"),
                    br#"{"n":1}"#,
                    false,
                    &Expect::default(),
                )
                .expect("indexed");
            lazy.mark_written(i, now);
            let max_seq_no = shard.max_seq_no();
            if lazy.due(&i, max_seq_no, PublishAge::Node, now) {
                shard.refresh().expect("refresh");
                let files = shard.committed_files().expect("files");
                publisher
                    .publish(&dirs[i], &files, 3, "node-1", max_seq_no, now)
                    .expect("published");
                lazy.published(i, max_seq_no);
                publishes = publishes.saturating_add(1);
            }
        }
    }
    (
        publishes,
        store.meter().count(S3Op::Put),
        store.meter().count(S3Op::List),
    )
}

#[test]
fn lazy_publishing_makes_a_fifth_of_the_publishes_at_the_requests_of_one_blob_and_one_swap() {
    let (eager, _, _) = run(0);
    let (lazy, puts, lists) = run(1_000);
    let shards = u64::try_from(SHARDS).expect("small");
    assert_eq!(eager, shards * 40, "age 0 publishes on every write");
    // The first publish at once, then one a period: the write after a publish
    // is the oldest unpublished, 250 ms later, and is due an age after that,
    // so at 0, 1.25, 2.5, ... 8.75 s: eight of an eager loop's forty.
    assert_eq!(lazy, shards * 8, "a fifth of the publishes");
    assert_eq!(puts, lazy * 2, "a blob and a swap each");
    assert_eq!(lists, 0, "a publish lists nothing");
}
