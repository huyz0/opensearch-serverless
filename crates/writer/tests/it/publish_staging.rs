//! Commit staging over the in-process emulator: which publishes write the
//! register and which keep their manifest, and what each makes of the store.

// ARITH: test sizes and request counts are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::time::Duration;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{CommitManifest, FileLocation, ShardRef};
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

fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn publisher(store: &Arc<Emulator>, staging: Option<Duration>) -> ShardPublisher {
    let mut p = ShardPublisher::new(Arc::clone(store) as Arc<dyn BlobStore>, "", &SHARD)
        .expect("publisher");
    p.use_staging(staging);
    p
}

fn puts(store: &Emulator) -> u64 {
    store.meter().count(S3Op::Put)
}

fn reads(store: &Emulator) -> u64 {
    let m = store.meter();
    m.count(S3Op::Get) + m.count(S3Op::GetRange) + m.count(S3Op::Head) + m.count(S3Op::List)
}

fn register(store: &Emulator) -> Option<(u64, Vec<u8>)> {
    let key = SHARD.manifest_key("").expect("key");
    let mut buf = vec![0u8; 1 << 20];
    let read = store.read_register(&key, &mut buf).expect("read")?;
    buf.truncate(read.len);
    Some((read.generation, buf))
}

/// Every blob a manifest names exists in the store.
fn assert_blobs_exist(store: &Emulator, manifest: &[u8]) {
    let parsed = CommitManifest::parse(manifest).expect("parses");
    for f in parsed.files().iter().filter(|f| f.length != Some(0)) {
        let key = match &f.location {
            FileLocation::Packed { term_dir, blob, .. } => {
                format!("{}{term_dir}/{blob}", SHARD.base_path("").expect("base"))
            }
            FileLocation::Whole { term_dir } => {
                format!(
                    "{}{term_dir}/{}",
                    SHARD.base_path("").expect("base"),
                    f.name
                )
            }
            other => panic!("unexpected location {other:?}"),
        };
        assert!(
            store.head(&key).expect("head").is_some(),
            "{} is named but missing at {key}",
            f.name
        );
    }
}

#[test]
fn with_staging_off_a_routine_publish_is_a_publish() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let (a, b) = (store(), store());
    let (mut routine, mut plain) = (publisher(&a, None), publisher(&b, None));
    for id in ["x", "y", "z"] {
        let files = commit(&mut s, id);
        let one = routine
            .publish_routine(&dir, &files, 3, "node-1", 4, 1)
            .expect("routine");
        let two = plain
            .publish(&dir, &files, 3, "node-1", 4, 1)
            .expect("plain");
        assert!(!one.staged);
        assert_eq!(one.manifest, two.manifest);
    }
    assert_eq!(puts(&a), puts(&b));
    assert_eq!(reads(&a), reads(&b));
}

#[test]
fn the_first_routine_publish_of_a_term_writes_the_register() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let files = commit(&mut s, "a");
    let first = p
        .publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    assert!(!first.staged);
    let (generation, bytes) = register(&st).expect("register written");
    assert_eq!(generation, first.generation);
    assert_eq!(bytes, first.manifest);
    // A publisher that starts a later term over a staged one's register writes too.
    let files = commit(&mut s, "b");
    let next_term = p
        .publish_routine(&dir, &files, 4, "node-1", 1, 2)
        .expect("next term");
    assert!(!next_term.staged, "a new term is a fence, never staged");
}

#[test]
fn a_routine_publish_inside_the_checkpoint_age_makes_one_blob_put_and_no_swap() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let files = commit(&mut s, "a");
    p.publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    let (generation, bytes) = register(&st).expect("register");
    let (before_puts, before_reads) = (puts(&st), reads(&st));
    let files = commit(&mut s, "b");
    let second = p
        .publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("second");
    assert!(second.staged);
    assert_eq!(second.generation, generation, "the register's generation");
    assert_eq!(second.blobs.len(), 1);
    assert_eq!(puts(&st), before_puts + 1, "the blob, no register swap");
    assert_eq!(reads(&st), before_reads, "nothing is read");
    assert_eq!(
        register(&st),
        Some((generation, bytes)),
        "register as it was"
    );
    assert_blobs_exist(&st, &second.manifest);
}

#[test]
fn staged_files_are_not_uploaded_again() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let files = commit(&mut s, "a");
    p.publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    let files = commit(&mut s, "b");
    let second = p
        .publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("second");
    let before = puts(&st);
    let files = commit(&mut s, "c");
    let third = p
        .publish_routine(&dir, &files, 3, "node-1", 2, 3)
        .expect("third");
    assert!(third.staged);
    assert_eq!(third.blobs.len(), 1, "only the third commit's segment");
    assert_eq!(puts(&st), before + 1);
    let two = CommitManifest::parse(&second.manifest).expect("parses");
    let three = CommitManifest::parse(&third.manifest).expect("parses");
    for f in two
        .files()
        .iter()
        .filter(|f| !f.name.starts_with("segments_"))
    {
        assert_eq!(
            three.file(&f.name).expect("inherited").location,
            f.location,
            "{}",
            f.name
        );
    }
    assert_blobs_exist(&st, &third.manifest);
}

#[test]
fn a_routine_publish_at_the_checkpoint_age_swaps_with_every_file() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(Duration::ZERO));
    let files = commit(&mut s, "a");
    let first = p
        .publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    let files = commit(&mut s, "b");
    let before = puts(&st);
    let second = p
        .publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("second");
    assert!(!second.staged, "a zero age is always due");
    assert_eq!(puts(&st), before + 2, "blob and swap");
    assert_eq!(second.generation, first.generation + 1);
    let (generation, bytes) = register(&st).expect("register");
    assert_eq!(generation, second.generation);
    assert_eq!(bytes, second.manifest);
    assert_blobs_exist(&st, &bytes);
}

#[test]
fn a_checkpoint_after_stages_writes_the_newest_with_one_put_and_no_upload() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    assert!(p.flush_due().expect("nothing").is_none(), "nothing staged");
    let files = commit(&mut s, "a");
    let first = p
        .publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    assert!(p.flush_due().expect("still nothing").is_none());
    let files = commit(&mut s, "b");
    let staged = p
        .publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("staged");
    assert!(staged.staged);
    assert!(p.flush_due().expect("not due").is_none(), "an hour away");
    let before = (puts(&st), reads(&st));
    p.use_staging(Some(Duration::ZERO));
    let flushed = p.flush_due().expect("due").expect("written");
    assert!(!flushed.staged);
    assert_eq!(flushed.manifest, staged.manifest);
    assert_eq!(flushed.generation, first.generation + 1);
    assert_eq!((puts(&st), reads(&st)), (before.0 + 1, before.1));
    assert_eq!(register(&st), Some((flushed.generation, staged.manifest)));
    assert!(p.flush_due().expect("cleared").is_none());
}

#[test]
fn a_full_publish_after_stages_writes_everything_and_uploads_nothing_twice() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let files = commit(&mut s, "a");
    p.publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    let files = commit(&mut s, "b");
    let staged = p
        .publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("staged");
    let before = puts(&st);
    let done = p.publish(&dir, &files, 3, "node-1", 1, 3).expect("full");
    assert!(!done.staged);
    assert!(done.blobs.is_empty(), "the staged files are held already");
    assert_eq!(puts(&st), before + 1, "the swap alone");
    let (_, bytes) = register(&st).expect("register");
    let held = CommitManifest::parse(&bytes).expect("parses");
    let was = CommitManifest::parse(&staged.manifest).expect("parses");
    for f in was.files() {
        assert_eq!(held.file(&f.name).expect("named").location, f.location);
    }
    assert!(p.flush_due().expect("cleared").is_none());
    assert_blobs_exist(&st, &bytes);
}

#[test]
fn a_staged_commit_past_the_pack_deadline_is_dropped_and_its_files_uploaded_again() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let files = commit(&mut s, "a");
    let first = p
        .publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    let files = commit(&mut s, "b");
    p.publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("staged");
    p.set_swap_deadline(Duration::ZERO);
    std::thread::sleep(Duration::from_millis(2));
    p.use_staging(Some(Duration::ZERO));
    let gone = p.flush_due();
    assert!(matches!(gone, Err(PublishError::PackExpired)), "{gone:?}");
    assert_eq!(
        register(&st).map(|r| r.0),
        Some(first.generation),
        "never written"
    );
    p.set_swap_deadline(Duration::from_secs(3_600));
    let before = puts(&st);
    let again = p
        .publish(&dir, &files, 3, "node-1", 1, 3)
        .expect("publish again");
    assert_eq!(again.blobs.len(), 1, "the dropped commit's segment again");
    assert!(
        puts(&st) >= before + 2,
        "the segment's blob and the swap, at least"
    );
    assert_blobs_exist(&st, &again.manifest);
}

#[test]
fn a_checkpoint_over_a_register_a_newer_term_holds_is_stale_and_leaves_nothing_staged() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let files = commit(&mut s, "a");
    p.publish_routine(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    let files = commit(&mut s, "b");
    p.publish_routine(&dir, &files, 3, "node-1", 1, 2)
        .expect("staged");
    // A successor of a newer term takes the register.
    let files = commit(&mut s, "c");
    publisher(&st, None)
        .publish(&dir, &files, 4, "node-2", 2, 3)
        .expect("successor");
    p.use_staging(Some(Duration::ZERO));
    let lost = p.flush_due();
    assert!(
        matches!(
            lost,
            Err(PublishError::Stale {
                term: 3,
                found: Some(4)
            })
        ),
        "{lost:?}"
    );
    assert!(p.flush_due().expect("cleared").is_none());
    assert!(!p.has_staged());
}

#[test]
fn the_stage_sequence_rises_with_every_commit_while_staging_is_on() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st, Some(LONG));
    let mut last = -1;
    for id in ["a", "b", "c", "d"] {
        let files = commit(&mut s, id);
        let out = p
            .publish_routine(&dir, &files, 3, "node-1", 0, 1)
            .expect("publish");
        let seq = CommitManifest::parse(&out.manifest)
            .expect("parses")
            .stage_sequence;
        assert!(seq > last, "{seq} after {last}");
        last = seq;
    }
    let off = publisher(&store(), None)
        .publish(&dir, &commit(&mut s, "e"), 3, "node-1", 0, 1)
        .expect("off");
    assert_eq!(
        CommitManifest::parse(&off.manifest)
            .expect("parses")
            .stage_sequence,
        -1,
        "off is as it was"
    );
}
