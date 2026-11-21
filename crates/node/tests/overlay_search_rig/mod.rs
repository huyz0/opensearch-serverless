//! A shard written and published to a directory a reader serves, for the
//! searches of a reader that holds an overlay or a freshness check.

#![allow(dead_code)]

use std::net::TcpListener;
use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore, PooledStore, S3Op};
use oss_format::ShardRef;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_node::search::{SearchService, start_reader};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;

use crate::support::{self, request, server_config};

pub const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

pub const DESCRIPTOR: &str = r#"{"name":"logs","uuid":"u1","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"}}}","settings":{}}"#;

/// A shard being written and published to a directory the reader also reads.
pub struct Writing<'d> {
    shard: Shard<'d>,
    dir: &'d ByteBuffersDirectory,
    publisher: ShardPublisher,
    ordinal: i64,
}

impl Writing<'_> {
    /// Indexes `ids`, refreshes, publishes, and gives the manifest a push
    /// would carry.
    pub fn publish(&mut self, ids: &[&str]) -> Vec<u8> {
        for id in ids {
            self.shard
                .index(id, br#"{"n":1}"#, false, &Expect::default())
                .expect("indexed");
        }
        self.shard.refresh().expect("refresh");
        let files = self.shard.committed_files().expect("files");
        self.ordinal = self.ordinal.saturating_add(1);
        self.publisher
            .publish(
                self.dir,
                &files,
                3,
                "node-1",
                self.ordinal,
                u64::try_from(self.ordinal).expect("small"),
            )
            .expect("published")
            .manifest
    }
}

pub fn reader_store(root: &std::path::Path) -> Arc<PooledStore<FsStore>> {
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&config),
        Pool::Transport,
        SMALL_BUFFER,
        16,
    )
    .expect("fits");
    Arc::new(PooledStore::new(
        FsStore::new(root).expect("store"),
        buffers,
    ))
}

pub fn counts(store: &PooledStore<FsStore>) -> [u64; 7] {
    S3Op::ALL.map(|op| store.store().meter().count(op))
}

pub fn gets(before: [u64; 7], now: [u64; 7], op: S3Op) -> u64 {
    let at = S3Op::ALL.iter().position(|o| *o == op).expect("op");
    now[at].saturating_sub(before[at])
}

pub fn total(body: &str) -> u64 {
    let at = body
        .find(r#""total":{"value":"#)
        .expect("a total")
        .saturating_add(17);
    body[at..]
        .split(',')
        .next()
        .and_then(|n| n.parse().ok())
        .expect("a number")
}

/// A reader over the files a writer publishes, and the writer.
#[allow(dead_code)]
pub struct Fixture<'d> {
    pub writing: Writing<'d>,
    pub store: Arc<PooledStore<FsStore>>,
    pub service: Arc<SearchService<FsStore>>,
    pub server: oss_rest::Server,
}

impl Fixture<'_> {
    pub fn search(&self) -> support::Reply {
        request(
            self.server.local_addr(),
            "POST",
            "/logs/_search",
            Some(br#"{"size":0,"track_total_hits":true,"query":{"match_all":{}}}"#),
        )
    }
}

pub fn fixture<'d>(
    root: &std::path::Path,
    dir: &'d ByteBuffersDirectory,
    first: &[&str],
) -> Fixture<'d> {
    fixture_as(root, dir, first, "reader-1")
}

/// A fixture whose reader is node `node`.
pub fn fixture_as<'d>(
    root: &std::path::Path,
    dir: &'d ByteBuffersDirectory,
    first: &[&str],
    node: &str,
) -> Fixture<'d> {
    let writer_store = Arc::new(FsStore::new(root).expect("store"));
    writer_store
        .create_register("indices/logs", DESCRIPTOR.as_bytes())
        .expect("descriptor");
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    let shard = Shard::new(dir, w, mapping, 3, Box::new(|| 0)).expect("shard");
    let shard_ref = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let publisher = ShardPublisher::new(
        Arc::clone(&writer_store) as Arc<dyn BlobStore>,
        "",
        &shard_ref,
    )
    .expect("publisher");
    let mut writing = Writing {
        shard,
        dir,
        publisher,
        ordinal: -1,
    };
    writing.publish(first);
    let store = reader_store(root);
    let budget = MemoryBudget::new(&support::budget_config());
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let (server, service) = start_reader(
        listener,
        server_config(),
        Arc::clone(&store),
        "",
        node,
        &budget,
    )
    .expect("starts");
    Fixture {
        writing,
        store,
        service,
        server,
    }
}

pub fn all(fx: &Fixture<'_>) -> u64 {
    let reply = fx.search();
    assert_eq!(reply.status, 200);
    total(&String::from_utf8_lossy(&reply.body))
}
