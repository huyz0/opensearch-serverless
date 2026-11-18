//! A reader over what a writer publishes: when it falls behind, the
//! writer's push takes the open index out of the set and the next search
//! opens on the pushed commit with one GET, where the register path takes
//! two; and what the writer replaced or deleted is not found.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore, PooledStore, S3Op};
use oss_format::ShardRef;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_node::push::PushHandler;
use oss_node::search::{SearchService, start_reader};
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

#[test]
fn a_push_moves_the_reader_onto_the_new_commit_with_one_get() {
    let root = TempDir::new("push-read");
    let dir = ByteBuffersDirectory::new();
    let mut f = fixture(root.path(), &dir, &["a", "b", "c"]);

    // Not open yet: a push is not kept, and says so.
    let second = f.writing.publish(&["d", "e"]);
    assert!(!f.service.take_push("logs", "u1", 0, &second));
    // The first search opens on the register, two GETs, and sees the
    // second commit already.
    let first = f.search();
    assert_eq!(first.status, 200, "{}", first.text());
    assert!(total(&first.text()) >= 5, "{}", first.text());

    // A third commit; its push finds the index open and behind.
    let third = f.writing.publish(&["f"]);
    assert!(f.service.take_push("logs", "u1", 0, &third));
    assert_eq!(f.service.open_indexes(), 0, "let go of the older commit");
    let before = counts(&f.store);
    let moved = f.search();
    let after = counts(&f.store);
    assert_eq!(moved.status, 200, "{}", moved.text());
    assert!(
        total(&moved.text()) > total(&first.text()),
        "{} then {}",
        first.text(),
        moved.text()
    );
    assert_eq!(
        gets(before, after, S3Op::Get),
        1,
        "the descriptor only, not the commit register"
    );
    for op in [S3Op::List, S3Op::Head, S3Op::Put] {
        assert_eq!(gets(before, after, op), 0, "{op:?}");
    }
    assert_eq!(f.service.open_indexes(), 1);
}

#[test]
fn a_push_that_changes_nothing_leaves_the_index_open() {
    let root = TempDir::new("push-same");
    let dir = ByteBuffersDirectory::new();
    let mut f = fixture(root.path(), &dir, &["a", "b"]);
    let second = f.writing.publish(&["c"]);
    assert_eq!(f.search().status, 200);
    assert_eq!(f.service.open_indexes(), 1);
    // The commit it is open on, again: not newer.
    assert!(f.service.take_push("logs", "u1", 0, &second));
    assert_eq!(f.service.open_indexes(), 1);
    // Another index's uuid, another shard, and bytes that are no commit.
    assert!(!f.service.take_push("logs", "other", 0, &second));
    assert!(!f.service.take_push("logs", "u1", 1, &second));
    assert!(!f.service.take_push("elsewhere", "u1", 0, &second));
    assert!(f.service.take_push("logs", "u1", 0, b"not a commit"));
    assert_eq!(f.service.open_indexes(), 1, "nothing was let go");
    let before = counts(&f.store);
    assert_eq!(f.search().status, 200);
    assert_eq!(counts(&f.store), before, "an open index makes no request");
}

#[test]
fn the_handler_answers_whether_the_shard_is_open_and_moves_the_reader() {
    use oss_transport::actions::push::{PushRequest, PushResponse};
    use oss_transport::frame::VariableHeader;
    use oss_transport::server::{Handler, Request};
    use oss_transport::stream::{Input, Output, TaskId};
    use oss_transport::version::WireVersion;

    let root = TempDir::new("push-handler");
    let dir = ByteBuffersDirectory::new();
    let mut f = fixture(root.path(), &dir, &["a"]);
    let handler = PushHandler::new(Arc::clone(&f.service));
    let ask = |commit: Vec<u8>| {
        let request = PushRequest {
            parent: TaskId::EMPTY,
            index_name: "logs".into(),
            index_uuid: "u1".into(),
            shard: 0,
            commit,
            ..PushRequest::default()
        };
        let mut message = Vec::new();
        request
            .write(&mut Output::new(&mut message, 1 << 20))
            .expect("encodes");
        let variable = VariableHeader::default();
        let mut answer = Vec::new();
        handler
            .handle(
                &Request {
                    action: "internal:serverless/commit/push",
                    variable: &variable,
                    message: &message,
                    version: WireVersion::SHELL,
                },
                &mut Output::new(&mut answer, 1 << 20),
            )
            .expect("answers");
        PushResponse::read(&mut Input::new(&answer))
            .expect("decodes")
            .open
    };
    let second = f.writing.publish(&["b"]);
    assert!(!ask(second), "not open yet");
    assert_eq!(f.search().status, 200);
    let third = f.writing.publish(&["c"]);
    assert!(ask(third), "open here");
    assert_eq!(
        f.service.open_indexes(),
        0,
        "and let go of the older commit"
    );

    // A message that does not decode is refused as a bad request.
    let variable = VariableHeader::default();
    let mut answer = Vec::new();
    let refused = handler.handle(
        &Request {
            action: "internal:serverless/commit/push",
            variable: &variable,
            message: b"\xff",
            version: WireVersion::SHELL,
        },
        &mut Output::new(&mut answer, 1 << 20),
    );
    assert!(refused.is_err());
}

#[test]
fn a_replaced_or_deleted_document_is_not_found() {
    let root = TempDir::new("soft-deletes");
    let dir = ByteBuffersDirectory::new();
    let mut f = fixture(root.path(), &dir, &["a", "b", "c"]);
    // `a` is written again and `b` is deleted: three live documents become
    // two, and the old versions stay in the segment as soft deletes.
    f.writing
        .shard
        .index("a", br#"{"n":2}"#, false, &Expect::default())
        .expect("replaced");
    f.writing
        .shard
        .delete("b", &Expect::default())
        .expect("deleted");
    f.writing.publish(&[]);
    let reply = f.search();
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(total(&reply.text()), 2, "{}", reply.text());
}
