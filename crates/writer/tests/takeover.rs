//! A node that won a shard's head replays what the previous writer logged
//! and adopts the shard into its register.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::committer::{Rig, rec, rig, shard, window};
use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::{CommitManifest, MAX_SHARD_HEAD_BYTES, ShardHead, ShardRef};
use oss_writer::index::Shard;
use oss_writer::log::NodeLogTakeover;
use oss_writer::mapping::Mapping;
use oss_writer::publish::{PublishError, ShardPublisher};
use oss_writer::takeover::{AdoptError, HeadWriters, Predecessors, Settled, ShardEngine, adopt};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn new_shard(dir: &ByteBuffersDirectory, term: i64) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    Shard::new(dir, w, mapping, term, Box::new(|| 0)).expect("shard")
}

fn head(n: u32, term: i64, writers: &[&str]) -> ShardHead {
    ShardHead {
        index: "logs".to_owned(),
        shard: n,
        term,
        owner_node_id: Some("node-2".to_owned()),
        owner_ephemeral_id: Some("e2".to_owned()),
        lease_expires_at_millis: 1_000_000,
        index_uuid: Some(format!("uuid-{n}")),
        writers: writers.iter().map(|w| (*w).to_owned()).collect(),
    }
}

fn put_head(r: &Rig, h: &ShardHead) {
    let key = shard_head_key("", &h.index, h.shard);
    r.store
        .create_register(&key, &h.to_bytes().expect("fits"))
        .expect("creates");
}

fn read_head(r: &Rig, n: u32) -> ShardHead {
    let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
    let read = r
        .store
        .read_register(&shard_head_key("", "logs", n), &mut buf)
        .expect("reads")
        .expect("exists");
    ShardHead::parse(&buf[..read.len]).expect("parses")
}

fn shard_ref(n: u32) -> (String, ShardRef<'static>) {
    let uuid = format!("uuid-{n}");
    let leaked: &'static str = Box::leak(uuid.clone().into_boxed_str());
    (
        uuid,
        ShardRef {
            index: "logs",
            uuid: leaked,
            shard: n,
        },
    )
}

fn publisher(r: &Rig, n: u32) -> ShardPublisher {
    let (_, sref) = shard_ref(n);
    ShardPublisher::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "", &sref).expect("publisher")
}

/// The previous writer's three records of shard `n`, fenced by this
/// successor's takeover, behind a factory that counts its calls.
fn predecessors(r: &Rig, calls: &Arc<AtomicU64>) -> Predecessors {
    let log = r.log.clone();
    let epochs = r.committer.positions();
    let calls = Arc::clone(calls);
    Predecessors::new(Box::new(move |_writer| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(NodeLogTakeover::new(
            log.clone(),
            epochs.clone(),
            "node-2",
            2,
        ))))
    }))
}

/// The fence a batch places once on each predecessor's epochs, before the
/// shards it takes are counted.
fn fenced(preds: &Predecessors) {
    preds
        .of("node-1")
        .expect("made")
        .expect("a log")
        .fence_all()
        .expect("fences");
}

fn write_three(r: &Rig, n: u32) {
    for seq in 0..3 {
        r.committer
            .commit(&shard(n), 1, &[rec(&format!("a{seq}"), seq)])
            .expect("lands");
    }
}

fn registered_term(r: &Rig, n: u32) -> u64 {
    let (_, sref) = shard_ref(n);
    let mut buf = vec![0u8; 1 << 20];
    let read = r
        .store
        .read_register(&sref.manifest_key("").expect("key"), &mut buf)
        .expect("reads")
        .expect("a commit is registered");
    CommitManifest::parse(&buf[..read.len])
        .expect("parses")
        .term
}

/// An engine that records what reaches it.
struct Recording {
    through: i64,
    seen: Vec<String>,
}

impl oss_writer::takeover::Engine for Recording {
    fn committed_through(&self) -> i64 {
        self.through
    }

    fn replay(&mut self, record: &oss_format::WalRecord<'_>) -> Result<bool, String> {
        self.seen.push(record.id().to_owned());
        Ok(true)
    }

    fn publish(&mut self, _term: u64) -> Result<oss_writer::publish::Published, PublishError> {
        Err(PublishError::Directory("not here".to_owned()))
    }
}

#[test]
fn only_records_above_the_commit_reach_the_engine_oldest_first() {
    let r = rig(window(0));
    write_three(&r, 1);
    write_three(&r, 2);
    let won = head(1, 2, &["node-1"]);
    let preds = predecessors(&r, &Arc::default());
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = Recording {
        through: 0,
        seen: Vec::new(),
    };
    // The publish is refused, which ends the adopt after the replay.
    let _ = adopt(&shard(1), &won, &preds, &mut engine, &heads);
    assert_eq!(
        engine.seen,
        ["a1", "a2"],
        "shard 1's records past seq 0 only"
    );
}

fn registered_wal_ordinal(r: &Rig, n: u32) -> i64 {
    let (_, sref) = shard_ref(n);
    let mut buf = vec![0u8; 1 << 20];
    let read = r
        .store
        .read_register(&sref.manifest_key("").expect("key"), &mut buf)
        .expect("reads")
        .expect("a commit is registered");
    CommitManifest::parse(&buf[..read.len])
        .expect("parses")
        .wal_ordinal
}

fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

#[test]
fn replayed_records_are_published_once_under_the_new_term_and_the_head_stops_naming_the_writer() {
    let r = rig(window(0));
    write_three(&r, 1);
    let won = head(1, 2, &["node-1"]);
    put_head(&r, &won);
    let calls = Arc::new(AtomicU64::new(0));
    let preds = predecessors(&r, &calls);
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    let mut p = publisher(&r, 1);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    fenced(&preds);
    let before = (puts(&r), r.store.meter().count(S3Op::List));
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.replayed, 3);
    assert!(done.published.is_some());
    assert_eq!(done.settled, Settled::Cleared);
    assert_eq!(registered_term(&r, 1), 2);
    assert!(
        registered_wal_ordinal(&r, 1) < 0,
        "the log position is not recorded"
    );
    assert_eq!(s.max_seq_no(), 2);
    assert!(read_head(&r, 1).writers.is_empty());
    assert_eq!(
        puts(&r) - before.0,
        3,
        "a segment blob, the register swap and the head swap"
    );
    assert_eq!(r.store.meter().count(S3Op::List), before.1, "no LIST");
}

#[test]
fn records_the_commit_holds_are_not_applied() {
    let r = rig(window(0));
    write_three(&r, 1);
    let won = head(1, 2, &["node-1"]);
    put_head(&r, &won);
    let preds = predecessors(&r, &Arc::default());
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    // The commit this node opened from holds a0.
    s.replay(&rec("a0", 0)).expect("replays");
    s.refresh().expect("commits");
    let mut p = publisher(&r, 1);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.replayed, 2, "a1 and a2");
}

#[test]
fn a_writer_with_no_log_is_still_named_by_the_head() {
    let r = rig(window(0));
    write_three(&r, 1);
    let won = head(1, 2, &["node-0", "node-1"]);
    put_head(&r, &won);
    let log = r.log.clone();
    let epochs = r.committer.positions();
    // node-0 has no lease: nothing is known of its log.
    let preds = Predecessors::new(Box::new(move |writer| {
        Ok((writer == "node-1").then(|| {
            Arc::new(NodeLogTakeover::new(
                log.clone(),
                epochs.clone(),
                "node-2",
                2,
            ))
        }))
    }));
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    let mut p = publisher(&r, 1);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.replayed, 3);
    assert_eq!(read_head(&r, 1).writers, ["node-0"], "only what was read");
}

#[test]
fn records_all_in_the_commit_publish_nothing() {
    let r = rig(window(0));
    write_three(&r, 1);
    let won = head(1, 2, &["node-1"]);
    put_head(&r, &won);
    let preds = predecessors(&r, &Arc::default());
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    for seq in 0..3 {
        s.replay(&rec(&format!("a{seq}"), seq)).expect("replays");
    }
    s.refresh().expect("commits");
    let mut p = publisher(&r, 1);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.replayed, 0);
    assert!(done.published.is_none());
    assert!(read_head(&r, 1).writers.is_empty(), "the log was read");
}

#[test]
fn a_shard_with_nothing_to_replay_publishes_nothing() {
    let r = rig(window(0));
    write_three(&r, 1);
    let preds = predecessors(&r, &Arc::default());
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    // Shard 3: the predecessor logged nothing of it.
    let won = head(3, 2, &["node-1"]);
    put_head(&r, &won);
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    let mut p = publisher(&r, 3);
    fenced(&preds);
    let before = puts(&r);
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(3), &won, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.replayed, 0);
    assert!(done.published.is_none());
    assert_eq!(puts(&r) - before, 1, "only the head swap");
    // A head naming no writer costs nothing at all.
    let free = head(4, 2, &[]);
    put_head(&r, &free);
    let dir4 = ByteBuffersDirectory::new();
    let mut s4 = new_shard(&dir4, 2);
    let mut p4 = publisher(&r, 4);
    let before = puts(&r);
    let mut engine = ShardEngine::new(&mut s4, &dir4, &mut p4, "node-2", 1);
    let done = adopt(&shard(4), &free, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.settled, Settled::NothingToClear);
    assert_eq!(puts(&r), before);
}

#[test]
fn a_batch_reads_each_predecessors_log_once_however_many_shards_it_takes() {
    let r = rig(window(0));
    write_three(&r, 1);
    write_three(&r, 2);
    let calls = Arc::new(AtomicU64::new(0));
    let preds = predecessors(&r, &calls);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    for n in [1, 2] {
        let won = head(n, 2, &["node-1"]);
        put_head(&r, &won);
        let dir = ByteBuffersDirectory::new();
        let mut s = new_shard(&dir, 2);
        let mut p = publisher(&r, n);
        let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
        let done = adopt(&shard(n), &won, &preds, &mut engine, &heads).expect("adopts");
        assert_eq!(done.replayed, 3, "shard {n}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one lease read");
}

#[test]
fn a_head_that_moved_on_is_not_written() {
    let r = rig(window(0));
    write_three(&r, 1);
    let won = head(1, 2, &["node-1"]);
    put_head(&r, &won);
    // Another node took the shard at term 3 meanwhile.
    let mut moved = head(1, 3, &["node-1", "node-2"]);
    moved.owner_node_id = Some("node-3".to_owned());
    let key = shard_head_key("", "logs", 1);
    let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
    let generation = r
        .store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("exists")
        .generation;
    r.store
        .swap_register(&key, generation, &moved.to_bytes().expect("fits"))
        .expect("swaps");
    let preds = predecessors(&r, &Arc::default());
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    let mut p = publisher(&r, 1);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads).expect("adopts");
    assert_eq!(done.settled, Settled::Lost);
    assert_eq!(read_head(&r, 1), moved, "not written");
}

#[test]
fn a_head_is_not_written_unless_it_is_still_this_nodes_at_this_term() {
    let r = rig(window(0));
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let key = shard_head_key("", "logs", 1);
    let won = head(1, 2, &["node-1"]);
    let current_generation = |r: &Rig| {
        let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
        r.store
            .read_register(&key, &mut buf)
            .expect("reads")
            .expect("exists")
            .generation
    };
    put_head(&r, &won);
    // The same term, another owner.
    let mut other_owner = head(1, 2, &["node-1"]);
    other_owner.owner_node_id = Some("node-3".to_owned());
    let generation = current_generation(&r);
    r.store
        .swap_register(&key, generation, &other_owner.to_bytes().expect("fits"))
        .expect("swaps");
    assert_eq!(
        heads.settle(&won, &["node-1".to_owned()]).expect("settles"),
        Settled::Lost
    );
    assert_eq!(read_head(&r, 1), other_owner);
    // This owner, a later term.
    let later = head(1, 3, &["node-1"]);
    let generation = current_generation(&r);
    r.store
        .swap_register(&key, generation, &later.to_bytes().expect("fits"))
        .expect("swaps");
    assert_eq!(
        heads.settle(&won, &["node-1".to_owned()]).expect("settles"),
        Settled::Lost
    );
    assert_eq!(read_head(&r, 1), later);
}

#[test]
fn a_newer_term_in_the_register_refuses_the_publish_and_the_head_keeps_its_writers() {
    let r = rig(window(0));
    write_three(&r, 1);
    // A successor already published this shard at term 3.
    let other_dir = ByteBuffersDirectory::new();
    let mut other = new_shard(&other_dir, 3);
    other
        .index("z", br#"{"n":9}"#, false, &Default::default())
        .expect("writes");
    other.refresh().expect("commits");
    let files = other.committed_files().expect("files");
    publisher(&r, 1)
        .publish(&other_dir, &files, 3, "node-3", -1, 1)
        .expect("published");
    let won = head(1, 2, &["node-1"]);
    put_head(&r, &won);
    let preds = predecessors(&r, &Arc::default());
    let dir = ByteBuffersDirectory::new();
    let mut s = new_shard(&dir, 2);
    let mut p = publisher(&r, 1);
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let err = adopt(&shard(1), &won, &preds, &mut engine, &heads).expect_err("refused");
    assert!(
        matches!(err, AdoptError::Publish(PublishError::Stale { .. })),
        "{err}"
    );
    assert_eq!(registered_term(&r, 1), 3);
    assert_eq!(read_head(&r, 1).writers, ["node-1"]);
}

#[test]
fn an_adopt_error_says_where_it_failed() {
    let replay = AdoptError::Replay("bad record".to_owned());
    assert_eq!(replay.to_string(), "applying a record: bad record");
    let stale = AdoptError::Publish(PublishError::Stale {
        term: 2,
        found: Some(3),
    });
    assert!(stale.to_string().starts_with("publishing the replay: "));
    let head = AdoptError::Head(oss_blob::BlobError::Unavailable);
    assert!(head.to_string().starts_with("the shard head: "));
}
