//! docs/specs/reader-overlay-source: a search served over the overlay an
//! installed source finished, and from the commit alone without one.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore, PooledStore, S3Op};
use oss_format::ShardRef;
use oss_format::WalRecord;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_node::search::{SearchService, start_reader};
use oss_node::tail_overlay::{TAIL_OVERLAY_ENABLED, install};
use oss_reader::commit_push::Stage;
use oss_reader::overlay::{CommitAccess, Overlay, OverlaySource, Ready, find_copies};
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
#[allow(dead_code)]
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
            Some(br#"{"size":0,"track_total_hits":true,"query":{"match_all":{}}}"#),
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

/// Builds the overlay of a replaced id, a deleted one and a new one on the
/// commit it is handed, as often as it is asked unless it has built one.
struct Source {
    mapping: Mapping,
    asked: Mutex<u32>,
    built: Mutex<Option<Arc<Ready>>>,
    ready: bool,
    /// The id of the third record: changing it makes the next overlay differ.
    new_id: Mutex<&'static str>,
    /// The shards an engine closed on, in order.
    closed: Mutex<Vec<(String, u32)>>,
}

impl Source {
    fn new(ready: bool) -> Arc<Self> {
        Arc::new(Self {
            mapping: Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#)
                .expect("mapping"),
            asked: Mutex::new(0),
            built: Mutex::new(None),
            ready,
            new_id: Mutex::new("c"),
            closed: Mutex::new(Vec::new()),
        })
    }
}

impl Source {
    /// The next `ready` builds an overlay whose new document has `id`.
    fn replace_with(&self, id: &'static str) {
        *self.new_id.lock().expect("lock") = id;
        *self.built.lock().expect("lock") = None;
    }

    fn views(&self) -> u64 {
        self.built
            .lock()
            .expect("lock")
            .as_ref()
            .map_or(0, |r| r.views_made())
    }
}

impl OverlaySource for Source {
    fn ready(
        &self,
        uuid: &str,
        shard: u32,
        _stage: &Stage,
        commit: &Arc<dyn CommitAccess>,
        _now: u64,
    ) -> Option<Arc<Ready>> {
        assert_eq!((uuid, shard), ("u1", 0));
        assert_eq!(
            commit.mapping_json(),
            r#"{"properties":{"n":{"type":"integer"}}}"#,
            "the source gets the mapping the descriptor holds"
        );
        let mut asked = self.asked.lock().expect("lock");
        *asked = asked.saturating_add(1);
        if !self.ready {
            return None;
        }
        let mut built = self.built.lock().expect("lock");
        if built.is_none() {
            let records = [
                WalRecord::index("a".to_owned(), r#"{"n":7}"#.to_owned(), 1000, 1, 1001),
                WalRecord::deletion("b".to_owned(), 1001, 1, 1002),
                WalRecord::index(
                    self.new_id.lock().expect("lock").to_owned(),
                    r#"{"n":9}"#.to_owned(),
                    1002,
                    1,
                    1003,
                ),
            ];
            let overlay = Overlay::build(&self.mapping, &records, 0, 4 << 20).expect("builds");
            let copies = find_copies(commit.reader(), overlay.ids()).expect("copies");
            *built = Some(Arc::new(Ready::new(overlay, copies)));
        }
        built.clone()
    }

    fn closed(&self, uuid: &str, shard: u32, _stage: &Stage, _now: u64) {
        self.closed
            .lock()
            .expect("lock")
            .push((uuid.to_owned(), shard));
    }
}

fn n_is(fx: &Fixture<'_>, n: i64) -> u64 {
    let reply = request(
        fx.server.local_addr(),
        "POST",
        "/logs/_search",
        Some(
            format!(r#"{{"size":0,"track_total_hits":true,"query":{{"term":{{"n":{n}}}}}}}"#)
                .as_bytes(),
        ),
    );
    assert_eq!(reply.status, 200);
    total(&String::from_utf8_lossy(&reply.body))
}

fn all(fx: &Fixture<'_>) -> u64 {
    let reply = fx.search();
    assert_eq!(reply.status, 200);
    total(&String::from_utf8_lossy(&reply.body))
}

#[test]
fn with_the_setting_off_no_source_is_installed_and_the_commit_answers() {
    let root = TempDir::new("overlay-off");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(!install(
        &fx.service,
        TAIL_OVERLAY_ENABLED.default(),
        source.clone()
    ));
    assert_eq!(all(&fx), 2);
    assert_eq!(*source.asked.lock().expect("lock"), 0);
}

#[test]
fn an_installed_source_s_overlay_replaces_deletes_and_adds_what_a_search_sees() {
    let root = TempDir::new("overlay-on");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    assert!(install(&fx.service, true, Source::new(true)));
    // a became n=7, b is deleted, c is new: a, c remain.
    assert_eq!(all(&fx), 2);
    assert_eq!(n_is(&fx, 7), 1);
    assert_eq!(n_is(&fx, 9), 1);
    assert_eq!(n_is(&fx, 1), 0);
}

#[test]
fn a_source_with_nothing_finished_leaves_the_commits_answer() {
    let root = TempDir::new("overlay-none");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(false);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(all(&fx), 2);
    assert_eq!(n_is(&fx, 1), 2);
    assert!(*source.asked.lock().expect("lock") >= 2);
}

#[test]
fn searches_between_two_overlays_share_one_view_and_make_no_request() {
    let root = TempDir::new("overlay-view");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(all(&fx), 2);
    let before = counts(&fx.store);
    for _ in 0..3 {
        assert_eq!(all(&fx), 2);
    }
    let after = counts(&fx.store);
    for op in [S3Op::Get, S3Op::List, S3Op::Head, S3Op::Put, S3Op::Delete] {
        assert_eq!(gets(before, after, op), 0, "{op:?}");
    }
    assert_eq!(source.views(), 1, "four searches made one view");
}

#[test]
fn a_new_overlay_from_the_source_makes_a_new_view_once() {
    let root = TempDir::new("overlay-swap");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(n_is(&fx, 9), 1);
    source.replace_with("d");
    for _ in 0..3 {
        assert_eq!(n_is(&fx, 9), 1);
    }
    assert_eq!(source.views(), 1, "the new overlay was viewed once");
}

#[test]
fn the_setting_is_off_by_default() {
    assert!(!TAIL_OVERLAY_ENABLED.default());
}

#[test]
fn a_source_is_told_when_the_engine_it_served_closes() {
    let root = TempDir::new("overlay-closed");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(all(&fx), 2);
    assert!(source.closed.lock().expect("lock").is_empty());
    let Fixture {
        service, server, ..
    } = fx;
    server.shutdown();
    drop(service);
    assert_eq!(
        *source.closed.lock().expect("lock"),
        [("u1".to_owned(), 0)],
        "told once, for the shard it served"
    );
}
