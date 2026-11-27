//! A node that won a shard's head replays what the previous writer logged
//! and adopts the shard into its register.

use crate::common;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::committer::{Rig, rec, rig, shard, window};
use common::takeover::*;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::MAX_SHARD_HEAD_BYTES;
use oss_writer::log::NodeLogTakeover;
use oss_writer::publish::PublishError;
use oss_writer::takeover::{AdoptError, HeadWriters, Predecessors, Settled, ShardEngine, adopt};

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
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
    fenced(&preds);
    // The headers are read once for the replay; the slices not at all.
    let reads = |r: &Rig| {
        r.store
            .meter()
            .count(S3Op::Get)
            .saturating_add(r.store.meter().count(S3Op::GetRange))
    };
    let before = reads(&r);
    let mut engine = ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1);
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
    // Two earlier blobs' headers and the head: no slice of the three.
    assert_eq!(reads(&r) - before, 3, "headers and the head only");
    assert_eq!(done.replayed, 0);
    assert!(done.published.is_none());
    assert!(read_head(&r, 1).writers.is_empty(), "the log was read");
    let first = reads(&r);
    // A second shard-less ask finds the headers kept: only the head read.
    let again = preds.of("node-1").expect("made").expect("a log");
    again
        .for_each_record_above(
            &shard(1),
            2,
            |_| Ok::<_, oss_writer::log::TakeoverError>(()),
        )
        .expect("reads");
    assert_eq!(reads(&r), first, "no slice or body is fetched");
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
    let done = adopt(&shard(3), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
    let done = adopt(&shard(4), &free, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
        let done = adopt(&shard(n), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
    let done = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX).expect("adopts");
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
    let err = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX).expect_err("refused");
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
