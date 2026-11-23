//! What the writer publishes the reader opens: documents indexed, deleted and
//! replaced come back through `open_shard` from the register and blobs the
//! publisher wrote.

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, FsStore, PooledStore};
use oss_format::ShardRef;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::TempDir;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::pack::{PackLedger, PackStore, PackWriter};
use oss_writer::publish::{PackPlacement, ShardPublisher};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn register(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

fn open(root: &std::path::Path) -> oss_reader::OpenShard<FsStore> {
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 4).expect("fits");
    let store = Arc::new(PooledStore::new(
        FsStore::new(root).expect("store"),
        buffers,
    ));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    oss_reader::open_shard(store, cache, "", "logs", 0).expect("opens")
}

#[test]
fn the_reader_opens_what_the_writer_published() {
    let root = TempDir::new("publish-read");
    let store = Arc::new(FsStore::new(root.path()).expect("store"));
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    let mut shard = Shard::new(&dir, w, mapping, 3, Box::new(|| 0)).expect("shard");

    // The index's descriptor, as the shell writes it.
    store
        .create_register(
            "indices/logs",
            &register(
                r#"{"name":"logs","uuid":"u1","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"}}}","settings":{}}"#,
            ),
        )
        .expect("descriptor");
    let shard_ref = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut publisher =
        ShardPublisher::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", &shard_ref)
            .expect("publisher");

    for id in ["a", "b", "c"] {
        shard
            .index(id, br#"{"n":1}"#, false, &Expect::default())
            .expect("indexed");
    }
    shard.refresh().expect("refresh");
    let files = shard.committed_files().expect("files");
    publisher
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
    assert_eq!(open(root.path()).reader.num_docs(), 3);

    // A delete and a replacement publish as another commit.
    shard.delete("a", &Expect::default()).expect("deleted");
    shard
        .index("b", br#"{"n":2}"#, false, &Expect::default())
        .expect("replaced");
    shard.refresh().expect("refresh");
    let files = shard.committed_files().expect("files");
    publisher
        .publish(&dir, &files, 3, "node-1", 1, 2)
        .expect("published");
    // Replaced and deleted documents are soft deleted: the reader sees them
    // through the soft-deletes wrapper, as the writer does: what is left is
    // `c` and the new `b`.
    let live = open(root.path())
        .reader
        .soft_deletes_wrapper("__soft_deletes")
        .expect("wrapped");
    assert_eq!(live.num_docs(), 2);
}

/// With node packs on, the reader opens a commit whose files all live in a
/// pack: the register, the descriptor and the pack are all it reads.
#[test]
fn the_reader_opens_a_commit_published_into_a_node_pack() {
    let root = TempDir::new("publish-read-pack");
    let store = Arc::new(FsStore::new(root.path()).expect("store"));
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    let mut shard = Shard::new(&dir, w, mapping, 3, Box::new(|| 0)).expect("shard");
    store
        .create_register(
            "indices/logs",
            &register(
                r#"{"name":"logs","uuid":"u1","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"}}}","settings":{}}"#,
            ),
        )
        .expect("descriptor");
    let shard_ref = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let dynstore = Arc::clone(&store) as Arc<dyn BlobStore>;
    let writer = Arc::new(PackWriter::new(
        PackStore::new(Arc::clone(&dynstore), "", "node-1"),
        Some(PackLedger::new(Arc::clone(&dynstore), "", "node-1").expect("ledger")),
        1,
        std::time::Duration::from_millis(1),
        64 << 20,
    ));
    let mut publisher = ShardPublisher::new(dynstore, "", &shard_ref).expect("publisher");
    publisher.use_packs(Some(PackPlacement {
        writer,
        index_name: "logs".to_owned(),
        index_uuid: "u1".to_owned(),
        shard: 0,
        max_blob_bytes: 8 << 20,
    }));
    for id in ["a", "b", "c"] {
        shard
            .index(id, br#"{"n":1}"#, false, &Expect::default())
            .expect("indexed");
    }
    shard.refresh().expect("refresh");
    let files = shard.committed_files().expect("files");
    let published = publisher
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
    assert!(published.blobs.is_empty(), "every file is in the pack");
    assert!(
        root.path().join("packs/node-1").is_dir(),
        "the pack is under the node's prefix"
    );
    assert_eq!(open(root.path()).reader.num_docs(), 3);
}
