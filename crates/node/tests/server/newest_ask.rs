//! docs/specs/reader-newest-ask: a cold open on a register that is behind
//! its staging writer asks the writer for the newer commit, over the same
//! two GETs; and the transport source that asks.

use crate::support;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore, PooledStore, S3Op};
use oss_format::ShardRef;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_node::newest_ask::{NEWEST_COMMIT_ASK, install};
use oss_node::search::{SearchService, start_reader};
use oss_reader::commit_push::NewestSource;
use oss_testkit::TempDir;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;

use support::{request, server_config};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const DESCRIPTOR: &str = r#"{"name":"logs","uuid":"u1","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"}}}","settings":{}}"#;

/// A shard being written and published to a directory the reader also reads.
struct Writing<'d> {
    shard: Shard<'d>,
    dir: &'d ByteBuffersDirectory,
    publisher: ShardPublisher,
    ordinal: i64,
}

impl Writing<'_> {
    /// Indexes `ids`, refreshes, publishes, and gives the manifest a push
    /// would carry.
    fn publish(&mut self, ids: &[&str]) -> Vec<u8> {
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

fn reader_store(root: &std::path::Path) -> Arc<PooledStore<FsStore>> {
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

fn counts(store: &PooledStore<FsStore>) -> [u64; 7] {
    S3Op::ALL.map(|op| store.store().meter().count(op))
}

fn gets(before: [u64; 7], now: [u64; 7], op: S3Op) -> u64 {
    let at = S3Op::ALL.iter().position(|o| *o == op).expect("op");
    now[at].saturating_sub(before[at])
}

fn total(body: &str) -> u64 {
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
struct Fixture<'d> {
    writing: Writing<'d>,
    store: Arc<PooledStore<FsStore>>,
    service: Arc<SearchService<FsStore>>,
    server: oss_rest::Server,
}

impl Fixture<'_> {
    fn search(&self) -> support::Reply {
        request(
            self.server.local_addr(),
            "POST",
            "/logs/_search",
            Some(br#"{"size":0}"#),
        )
    }
}

fn fixture<'d>(
    root: &std::path::Path,
    dir: &'d ByteBuffersDirectory,
    first: &[&str],
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
        "reader-1",
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

/// Answers what it was told to, and says how often it was asked.
struct Source {
    answer: Mutex<Option<Vec<u8>>>,
    asked: Mutex<Vec<String>>,
}

impl Source {
    fn new(answer: Option<Vec<u8>>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            asked: Mutex::new(Vec::new()),
        })
    }
}

impl NewestSource for Source {
    fn newest(&self, index: &str, uuid: &str, shard: u32, writer: &str) -> Option<Vec<u8>> {
        assert_eq!((index, uuid, shard), ("logs", "u1", 0));
        self.asked.lock().expect("lock").push(writer.to_owned());
        self.answer.lock().expect("lock").clone()
    }
}

/// Publishes a first commit of `first`, then a second adding `second`, and
/// puts the register back on the first: the checkpoint a staging writer
/// leaves behind. Gives both manifests.
fn behind<'d>(f: &mut Fixture<'d>, first: &First, second: &[&str]) -> Vec<u8> {
    let newest = f.writing.publish(second);
    let key = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    }
    .manifest_key("")
    .expect("key");
    let mut buf = vec![0u8; 1 << 20];
    let read = f
        .store
        .store()
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("present");
    let swapped = f
        .store
        .store()
        .swap_register(&key, read.generation, &first.value)
        .expect("swaps");
    assert!(
        matches!(swapped, oss_blob::SwapOutcome::Applied(_)),
        "the register is put back on the first commit"
    );
    staged(&newest, 6)
}

/// The register after the first publish: the blob, and the commit in it.
struct First {
    value: Vec<u8>,
}

/// `manifest` with a stage sequence of `sequence`: the Rust publisher does
/// not stage yet, so the commits a staging writer would make are made here.
fn staged(manifest: &[u8], sequence: i64) -> Vec<u8> {
    let text = std::str::from_utf8(manifest).expect("json");
    let body = text.strip_suffix('}').expect("an object");
    format!("{body},\"stage_seq\":{sequence}}}").into_bytes()
}

fn open_on_first<'d>(
    root: &std::path::Path,
    dir: &'d ByteBuffersDirectory,
) -> (Fixture<'d>, First) {
    let f = fixture(root, dir, &["a", "b"]);
    // The register's value now, as a push would carry it.
    let key = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    }
    .manifest_key("")
    .expect("key");
    let mut buf = vec![0u8; 1 << 20];
    let read = f
        .store
        .store()
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("present");
    // The register's value, a commit manifest, staged as sequence 4.
    let value = staged(&buf[..read.len], 4);
    (f, First { value })
}

#[test]
fn a_cold_open_takes_the_newer_commit_its_writer_staged_with_the_same_gets() {
    let root = TempDir::new("newest-ask");
    let dir = ByteBuffersDirectory::new();
    let (mut f, first) = open_on_first(root.path(), &dir);
    let newest = behind(&mut f, &first, &["c", "d"]);
    let source = Source::new(Some(newest));
    assert!(
        !install(&f.service, false, source.clone()),
        "off installs nothing"
    );
    assert!(install(&f.service, true, source.clone()));
    assert!(!install(&f.service, true, source.clone()), "once");
    let before = counts(&f.store);
    let reply = f.search();
    let after = counts(&f.store);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(total(&reply.text()), 4, "{}", reply.text());
    assert_eq!(*source.asked.lock().expect("lock"), ["node-1"]);
    assert_eq!(gets(before, after, S3Op::Get), 2, "the register path's two");
    for op in [S3Op::List, S3Op::Head, S3Op::Put] {
        assert_eq!(gets(before, after, op), 0, "{op:?}");
    }
}

#[test]
fn no_answer_or_an_older_commit_leaves_the_open_on_the_register() {
    for kind in 0..3 {
        let root = TempDir::new("newest-none");
        let dir = ByteBuffersDirectory::new();
        let (mut f, first) = open_on_first(root.path(), &dir);
        behind(&mut f, &first, &["c", "d"]);
        let answer = match kind {
            0 => None,
            1 => Some(b"not a commit".to_vec()),
            // The register's own commit: not a later stage.
            _ => Some(first.value.clone()),
        };
        let source = Source::new(answer);
        f.service.set_newest_source(source.clone());
        let reply = f.search();
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert_eq!(total(&reply.text()), 2, "{kind}: {}", reply.text());
        assert_eq!(source.asked.lock().expect("lock").len(), 1);
    }
}

/// A source that takes as long as the transport source may, then has
/// nothing.
struct Slow(Duration);

impl NewestSource for Slow {
    fn newest(&self, _: &str, _: &str, _: u32, _: &str) -> Option<Vec<u8>> {
        std::thread::sleep(self.0);
        None
    }
}

#[test]
fn a_source_that_takes_the_whole_timeout_costs_a_cold_open_that_long_and_no_more() {
    let root = TempDir::new("newest-slow");
    let dir = ByteBuffersDirectory::new();
    let (mut f, first) = open_on_first(root.path(), &dir);
    behind(&mut f, &first, &["c", "d"]);
    f.service
        .set_newest_source(Arc::new(Slow(oss_node::newest_ask::ASK_TIMEOUT)));
    let started = std::time::Instant::now();
    let reply = f.search();
    let took = started.elapsed();
    assert_eq!(total(&reply.text()), 2, "the register's documents");
    assert!(took >= oss_node::newest_ask::ASK_TIMEOUT, "{took:?}");
    assert!(
        took < oss_node::newest_ask::ASK_TIMEOUT.saturating_mul(4),
        "{took:?}"
    );
    // The open index is kept: the next search asks nothing.
    let again = std::time::Instant::now();
    assert_eq!(f.search().status, 200);
    assert!(again.elapsed() < oss_node::newest_ask::ASK_TIMEOUT);
}

#[test]
fn without_a_source_a_cold_open_is_as_before() {
    let root = TempDir::new("newest-off");
    let dir = ByteBuffersDirectory::new();
    let (mut f, first) = open_on_first(root.path(), &dir);
    behind(&mut f, &first, &["c"]);
    let reply = f.search();
    assert_eq!(total(&reply.text()), 2, "{}", reply.text());
    assert!(
        !NEWEST_COMMIT_ASK.default(),
        "off until a cold open's latency is measured"
    );
}
