//! Adopting a predecessor's staged commit into the publisher (docs/specs/
//! open-from-logged-commit): the commit rides the successor's own publish,
//! and a lost swap is never retried over a register that moved.

// ARITH: test sizes and request counts are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::time::Duration;

use oss_writer::boot_clock::BootInstant;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{CommitManifest, ShardRef};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::{PublishError, ShardPublisher};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const SHARD: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 0,
};

const LONG: Duration = Duration::from_secs(3_600);

fn shard(dir: &ByteBuffersDirectory) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    Shard::new(dir, w, mapping, 3, Box::new(|| 0)).expect("shard")
}

fn commit(s: &mut Shard<'_>, id: &str) -> Vec<String> {
    s.index(id, br#"{"n":1}"#, false, &Expect::default())
        .expect("indexed");
    s.refresh().expect("refresh");
    s.committed_files().expect("files")
}

fn publisher(store: &Arc<Emulator>, staging: bool) -> ShardPublisher {
    let mut p = ShardPublisher::new(Arc::clone(store) as Arc<dyn BlobStore>, "", &SHARD)
        .expect("publisher");
    p.use_staging(staging.then_some(LONG));
    p
}

fn register(store: &Emulator) -> (u64, Vec<u8>) {
    let key = SHARD.manifest_key("").expect("key");
    let mut buf = vec![0u8; 1 << 20];
    let read = store
        .read_register(&key, &mut buf)
        .expect("read")
        .expect("some");
    buf.truncate(read.len);
    (read.generation, buf)
}

/// node-1 at term 3: `a` in the register, `b` staged. Returns the staged
/// manifest and the register's generation and bytes.
struct Left {
    staged: Vec<u8>,
    files: Vec<String>,
}

fn predecessor(store: &Arc<Emulator>, dir: &ByteBuffersDirectory, s: &mut Shard<'_>) -> Left {
    let mut one = publisher(store, true);
    let a = commit(s, "a");
    assert!(
        !one.publish(dir, &a, 3, "node-1", 1, 1)
            .expect("full")
            .staged
    );
    let b = commit(s, "b");
    let done = one
        .publish_routine(dir, &b, 3, "node-1", 2, 1)
        .expect("routine");
    assert!(done.staged, "b is staged");
    Left {
        staged: done.manifest,
        files: b,
    }
}

fn puts(store: &Emulator) -> u64 {
    store.meter().count(S3Op::Put)
}

#[test]
fn an_adopted_commit_is_swapped_in_by_the_successors_publish_with_no_upload() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let left = predecessor(&store, &dir, &mut s);
    let (generation, before) = register(&store);
    let mut two = publisher(&store, false);
    two.adopt_commit(&left.staged, generation, BootInstant::now())
        .expect("adopts");
    let puts_before = puts(&store);
    let done = two
        .publish(&dir, &left.files, 4, "node-2", -1, 2)
        .expect("publishes");
    assert_eq!(puts(&store) - puts_before, 1, "the swap alone");
    assert!(done.blobs.is_empty(), "nothing uploaded again");
    let (after, bytes) = register(&store);
    assert!(after > generation);
    assert_ne!(bytes, before);
    let manifest = CommitManifest::parse(&bytes).expect("parses");
    assert_eq!(manifest.term, 4, "written under the successor's term");
    assert!(manifest.file(&left.files[0]).is_some());
}

#[test]
fn a_swap_lost_after_adopting_is_not_retried_over_the_register_that_moved() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let left = predecessor(&store, &dir, &mut s);
    let (generation, _) = register(&store);
    let mut two = publisher(&store, false);
    two.adopt_commit(&left.staged, generation, BootInstant::now())
        .expect("adopts");
    // The returning predecessor writes its checkpoint first.
    let mut returned = publisher(&store, false);
    let c = commit(&mut s, "c");
    returned
        .publish(&dir, &c, 3, "node-1", 3, 3)
        .expect("a full publish");
    let (moved, theirs) = register(&store);
    assert!(moved > generation);
    let lost = two.publish(&dir, &left.files, 4, "node-2", -1, 4);
    assert!(matches!(lost, Err(PublishError::AdoptionLost)), "{lost:?}");
    assert_eq!(register(&store), (moved, theirs), "the register is theirs");
}

#[test]
fn an_adopted_commit_past_the_pack_deadline_is_not_swapped_in() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let left = predecessor(&store, &dir, &mut s);
    let (generation, bytes) = register(&store);
    let mut two = publisher(&store, false);
    two.set_swap_deadline(Duration::from_millis(50));
    let landed = BootInstant::now();
    two.adopt_commit(&left.staged, generation, landed)
        .expect("adopts");
    std::thread::sleep(Duration::from_millis(80));
    let late = two.publish(&dir, &left.files, 4, "node-2", -1, 2);
    assert!(matches!(late, Err(PublishError::PackExpired)), "{late:?}");
    assert_eq!(register(&store), (generation, bytes), "nothing was swapped");
}

#[test]
fn staged_sequences_continue_above_the_adopted_commit() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let left = predecessor(&store, &dir, &mut s);
    let staged_seq = CommitManifest::parse(&left.staged)
        .expect("parses")
        .stage_sequence;
    assert!(staged_seq >= 0);
    let (generation, _) = register(&store);
    let mut two = publisher(&store, true);
    two.adopt_commit(&left.staged, generation, BootInstant::now())
        .expect("adopts");
    two.publish(&dir, &left.files, 4, "node-2", -1, 2)
        .expect("publishes");
    let (_, bytes) = register(&store);
    let seq = CommitManifest::parse(&bytes)
        .expect("parses")
        .stage_sequence;
    assert!(seq > staged_seq, "{seq} should exceed {staged_seq}");
}

#[test]
fn a_publisher_made_again_in_the_same_term_continues_the_registers_stage_sequence() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let left = predecessor(&store, &dir, &mut s);
    let (_, bytes) = register(&store);
    let held = CommitManifest::parse(&bytes)
        .expect("parses")
        .stage_sequence;
    assert!(held >= 0);
    // The same node and term, a publisher made anew: it starts counting from
    // what the register holds, or a logged commit left behind would look newer.
    let mut again = publisher(&store, true);
    again
        .publish(&dir, &left.files, 3, "node-1", 4, 5)
        .expect("publishes");
    let (_, bytes) = register(&store);
    let now = CommitManifest::parse(&bytes)
        .expect("parses")
        .stage_sequence;
    assert!(now > held, "{now} should exceed {held}");
}

#[test]
fn a_dropped_adoption_builds_on_the_register_as_it_is() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let left = predecessor(&store, &dir, &mut s);
    let (generation, _) = register(&store);
    let mut two = publisher(&store, false);
    two.adopt_commit(&left.staged, generation, BootInstant::now())
        .expect("adopts");
    let mut returned = publisher(&store, false);
    let c = commit(&mut s, "c");
    returned
        .publish(&dir, &c, 3, "node-1", 3, 3)
        .expect("a full publish");
    two.drop_adopted();
    assert!(!two.has_adopted());
    let done = two
        .publish(&dir, &c, 4, "node-2", -1, 4)
        .expect("publishes");
    assert!(!done.superseded);
    let (_, bytes) = register(&store);
    assert_eq!(CommitManifest::parse(&bytes).expect("parses").term, 4);
}
