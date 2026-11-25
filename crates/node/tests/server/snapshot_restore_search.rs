//! docs/specs/snapshot-restore AC-1: an index a real shard published is
//! taken, restored under a new name, and a reader over the same store answers
//! a search over the restored index with what the snapshot captured.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use crate::support;

use std::borrow::Cow;
use std::net::TcpListener;
use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, FsStore, PooledStore};
use oss_format::{IndexRecord, ShardRef};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::index_delete::{DeleteService, NoSnapshots};
use oss_node::search::start_reader;
use oss_node::snapshot_restore_route::{SnapshotRestoreService, routes as restore_routes};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_node::snapshot_take_route::SnapshotTakeService;
use oss_rest::Router;
use oss_testkit::TempDir;
use oss_writer::index::{Expect, MergeGate, MergeGuard, MergeSettings, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;

use crate::snapshot_take::TAKE;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

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

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("directory");
    for entry in std::fs::read_dir(from).expect("reads").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copies");
        }
    }
}

/// The snapshot and restore routes over `blobs`, on a server of their own.
fn admin(
    blobs: &Arc<dyn BlobStore>,
    descriptors: impl Fn() -> DescriptorStore,
) -> oss_rest::Server {
    let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
    let service = Arc::new(SnapshotService::new(
        Arc::clone(blobs),
        "",
        "node-1",
        Arc::clone(&clock),
    ));
    let taker = Arc::new(SnapshotTakeService::new(
        Arc::clone(blobs),
        "",
        descriptors(),
        Arc::clone(&clock),
    ));
    let deletes = Arc::new(
        DeleteService::new(
            blobs,
            "",
            DeleteMode::Conditional,
            Arc::clone(&clock),
            Arc::new(NoSnapshots),
            1 << 16,
            8,
        )
        .unwrap(),
    );
    let restorer = Arc::new(SnapshotRestoreService::new(
        Arc::clone(blobs),
        "",
        descriptors(),
        deletes,
        clock,
        true,
        1 << 16,
    ));
    let router = routes(Router::new(), &service, Some(&taker)).expect("routes");
    let router = restore_routes(router, restorer).expect("restore routes");
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    oss_rest::Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &budget,
    )
    .expect("starts")
}

#[test]
fn a_restored_index_answers_a_search_with_what_the_snapshot_captured() {
    let root = TempDir::new("restore-search");
    let store = Arc::new(FsStore::new(root.path()).expect("store"));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors =
        || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();

    // An index of one shard with three documents, published as a writer does.
    let mut record = IndexRecord::new("logs", "u1", 1);
    record.mapping = Some(Cow::Borrowed(MAPPING));
    descriptors()
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(
        &dir,
        w,
        Mapping::parse(MAPPING.as_bytes()).expect("mapping"),
        3,
        Box::new(|| 0),
    )
    .expect("shard");
    for id in ["a", "b", "c"] {
        shard
            .index(id, br#"{"n":1}"#, false, &Expect::default())
            .expect("indexed");
    }
    shard.refresh().expect("refresh");
    let files = shard.committed_files().expect("files");
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    ShardPublisher::new(Arc::clone(&blobs), "", &at)
        .expect("publisher")
        .publish(&dir, &files, 3, "node-1", 0, 0)
        .expect("published");

    // Snapshot it and restore it as `copy`, over REST.
    let admin = admin(&blobs, descriptors);
    let call = |method: &str, target: &str, body: Option<&str>| {
        support::request(admin.local_addr(), method, target, body.map(str::as_bytes))
    };
    let repo =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(call("PUT", "/_snapshot/backups", Some(repo)).status, 200);
    let made = call(
        "PUT",
        "/_snapshot/backups/daily?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    // For the Java check over a Rust take (fixtures/snapshots/README.md): the
    // directory as the take left it, before the restore adds to it.
    if let Some(out) = std::env::var_os("OSS_WRITE_BUCKET") {
        copy_dir(root.path(), std::path::Path::new(&out));
    }
    let restored = call(
        "POST",
        "/_snapshot/backups/daily/_restore?wait_for_completion=true",
        Some(r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#),
    );
    assert_eq!(restored.status, 200, "{}", restored.text());

    // A reader over the same store answers over both.
    let reader_config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&reader_config),
        Pool::Transport,
        SMALL_BUFFER,
        16,
    )
    .expect("fits");
    let reading = Arc::new(PooledStore::new(
        FsStore::new(root.path()).expect("store"),
        buffers,
    ));
    let reader_budget = MemoryBudget::new(&support::budget_config());
    let (reader, _service) = start_reader(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        reading,
        "",
        "reader-1",
        &reader_budget,
    )
    .expect("starts");
    for name in ["logs", "copy"] {
        let found = support::request(
            reader.local_addr(),
            "POST",
            &format!("/{name}/_search"),
            Some(br#"{"size":0}"#),
        );
        assert_eq!(found.status, 200, "{name}: {}", found.text());
        assert_eq!(total(&found.text()), 3, "{name}: {}", found.text());
    }
    // What a crash part-way leaves, a descriptor that still carries the
    // restore's mark: the reader refuses it and answers nothing from it.
    let mut stuck = IndexRecord::new("stuck", "u9", 1);
    let marked = oss_format::mark_restoring(None).expect("marked");
    stuck.settings = &marked;
    descriptors()
        .create(&stuck, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
    let refused = support::request(
        reader.local_addr(),
        "POST",
        "/stuck/_search",
        Some(br#"{"size":0}"#),
    );
    assert_eq!(refused.status, 503, "{}", refused.text());
    assert!(
        refused.text().contains("cluster_block_exception")
            && refused.text().contains("restore it again"),
        "{}",
        refused.text()
    );
    // And the restored index kept none of the mark.
    let mut buf = vec![0u8; 1 << 16];
    let kept = descriptors()
        .get("copy", &mut buf)
        .expect("reads")
        .expect("there");
    assert!(!oss_format::is_restoring(kept.settings));
}

struct Open;

impl MergeGate for Open {
    fn try_start(&self, _: u64) -> Option<Box<dyn MergeGuard>> {
        Some(Box::new(()))
    }
}

// docs/specs/writer-segment-merge AC-10: a snapshot taken before a merging
// writer's later commits restores what it captured, after those commits
// merged away the segments it names.
#[test]
fn a_snapshot_taken_before_merges_restores_what_it_captured_after_them() {
    let root = TempDir::new("restore-merged");
    let store = Arc::new(FsStore::new(root.path()).expect("store"));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors =
        || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
    let mut record = IndexRecord::new("logs", "u1", 1);
    record.mapping = Some(Cow::Borrowed(MAPPING));
    descriptors()
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(
        &dir,
        w,
        Mapping::parse(MAPPING.as_bytes()).expect("mapping"),
        3,
        Box::new(|| 0),
    )
    .expect("shard")
    .with_merging(
        MergeSettings {
            segments_per_tier: 2,
            max_merge_at_once: 4,
            max_merged_segment_bytes: 64 << 20,
        },
        Arc::new(Open),
    );
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut publisher = ShardPublisher::new(Arc::clone(&blobs), "", &at).expect("publisher");
    let mut round = |shard: &mut Shard<'_>, id: usize| {
        shard
            .index(&id.to_string(), br#"{"n":1}"#, false, &Expect::default())
            .expect("indexed");
        shard.refresh().expect("refresh");
        let files = shard.committed_files().expect("files");
        publisher
            .publish(
                &dir,
                &files,
                3,
                "node-1",
                i64::try_from(id).expect("small"),
                0,
            )
            .expect("published");
    };
    for id in 0..3 {
        round(&mut shard, id);
    }
    let admin = admin(&blobs, descriptors);
    let call = |method: &str, target: &str, body: Option<&str>| {
        support::request(admin.local_addr(), method, target, body.map(str::as_bytes))
    };
    let repo =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(call("PUT", "/_snapshot/backups", Some(repo)).status, 200);
    let made = call(
        "PUT",
        "/_snapshot/backups/daily?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    for id in 3..24 {
        round(&mut shard, id);
    }
    let segments = lucene_search::directory_reader::DirectoryReader::open(&dir)
        .expect("opens")
        .segment_readers()
        .len();
    assert!(segments < 12, "merging ran: {segments} segments");
    let restored = call(
        "POST",
        "/_snapshot/backups/daily/_restore?wait_for_completion=true",
        Some(r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#),
    );
    assert_eq!(restored.status, 200, "{}", restored.text());
    let reader_config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = BufferPool::new(
        &MemoryBudget::new(&reader_config),
        Pool::Transport,
        SMALL_BUFFER,
        16,
    )
    .expect("fits");
    let reading = Arc::new(PooledStore::new(
        FsStore::new(root.path()).expect("store"),
        buffers,
    ));
    let reader_budget = MemoryBudget::new(&support::budget_config());
    let (reader, _service) = start_reader(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        reading,
        "",
        "reader-1",
        &reader_budget,
    )
    .expect("starts");
    for (name, want) in [("logs", 24), ("copy", 3)] {
        let found = support::request(
            reader.local_addr(),
            "POST",
            &format!("/{name}/_search"),
            Some(br#"{"size":0}"#),
        );
        assert_eq!(found.status, 200, "{name}: {}", found.text());
        assert_eq!(total(&found.text()), want, "{name}: {}", found.text());
    }
}
