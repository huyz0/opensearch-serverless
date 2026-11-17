//! Publishing a shard's commits over the in-process emulator: the requests a
//! publish makes, which files it uploads, the blob names, and the fence.

use std::sync::Arc;

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

fn publisher(store: &Arc<Emulator>) -> ShardPublisher {
    ShardPublisher::new(Arc::clone(store) as Arc<dyn BlobStore>, "", &SHARD).expect("publisher")
}

fn count(store: &Emulator, op: S3Op) -> u64 {
    store.meter().count(op)
}

#[test]
fn a_new_segment_is_one_blob_and_one_swap_with_no_reads_after_the_first() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st);
    let files = commit(&mut s, "a");
    let first = p.publish(&dir, &files, 3, "node-1", 0, 1).expect("first");
    assert_eq!(first.blobs.len(), 1);
    let puts = count(&st, S3Op::Put);
    let reads = count(&st, S3Op::Get) + count(&st, S3Op::GetRange) + count(&st, S3Op::Head);
    let files = commit(&mut s, "b");
    let second = p.publish(&dir, &files, 3, "node-1", 1, 2).expect("second");
    assert_eq!(second.blobs.len(), 1);
    assert_eq!(count(&st, S3Op::Put), puts + 2, "one blob PUT and one swap");
    assert_eq!(
        count(&st, S3Op::Get) + count(&st, S3Op::GetRange) + count(&st, S3Op::Head),
        reads,
        "no reads"
    );
    assert_eq!(count(&st, S3Op::List), 0);
    assert_eq!(second.generation, first.generation + 1);
}

#[test]
fn only_fresh_files_are_uploaded_and_inherited_locations_are_kept() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st);
    let first_files = commit(&mut s, "a");
    let first = p
        .publish(&dir, &first_files, 3, "node-1", 0, 1)
        .expect("first");
    let files = commit(&mut s, "b");
    let second = p.publish(&dir, &files, 3, "node-1", 1, 2).expect("second");
    let one = CommitManifest::parse(&first.manifest).expect("parses");
    let two = CommitManifest::parse(&second.manifest).expect("parses");
    for f in one
        .files()
        .iter()
        .filter(|f| !f.name.starts_with("segments_"))
    {
        let kept = two.file(&f.name).expect("inherited");
        assert_eq!(kept.location, f.location, "{}", f.name);
    }
    assert!(two.files().len() > one.files().len());
    assert_eq!(two.term, 3);
    assert_eq!(two.writer.as_deref(), Some("node-1"));
    assert_eq!(two.wal_ordinal, 1);
}

#[test]
fn a_taken_blob_name_gets_a_unique_one() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let files = commit(&mut s, "a");
    let first = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    // A second publisher of the same shard and term finds the plain name taken.
    let again = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 2)
        .expect("again");
    assert_ne!(first.blobs, again.blobs);
    assert!(again.blobs.iter().all(|b| b.contains('-')));
}

#[test]
fn a_newer_term_fences_an_older_one_and_a_second_writer_of_a_term_is_foreign() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let files = commit(&mut s, "a");
    publisher(&st)
        .publish(&dir, &files, 5, "node-1", 0, 1)
        .expect("held");
    let stale = publisher(&st).publish(&dir, &files, 4, "node-2", 0, 2);
    assert!(
        matches!(
            stale,
            Err(PublishError::Stale {
                term: 4,
                found: Some(5)
            })
        ),
        "{stale:?}"
    );
    let foreign = publisher(&st).publish(&dir, &files, 5, "node-2", 0, 3);
    assert!(
        matches!(foreign, Err(PublishError::Foreign { .. })),
        "{foreign:?}"
    );
}

#[test]
fn a_lost_swap_reads_the_register_once_and_publishes_again() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st);
    let files = commit(&mut s, "a");
    p.publish(&dir, &files, 3, "node-1", 0, 1).expect("first");
    // Another publisher of the same term and node moves the register on.
    let mut rival = publisher(&st);
    let files2 = commit(&mut s, "b");
    let rival_out = rival
        .publish(&dir, &files2, 3, "node-1", 1, 2)
        .expect("rival");
    let files3 = commit(&mut s, "c");
    let reads = count(&st, S3Op::Get) + count(&st, S3Op::GetRange);
    let out = p.publish(&dir, &files3, 3, "node-1", 2, 3).expect("retry");
    assert!(out.generation > rival_out.generation);
    assert_eq!(
        count(&st, S3Op::Get) + count(&st, S3Op::GetRange),
        reads + 2,
        "the lost swap's own generation probe and one register read"
    );
}

#[test]
fn a_manifest_over_the_first_read_buffer_is_read_whole() {
    use lucene_store::{DataOutput, Directory};
    let dir = ByteBuffersDirectory::new();
    let mut files = Vec::new();
    for i in 0..8_000 {
        let name = format!("_{i}.cfs");
        let mut out = dir.create_output(&name).expect("output");
        out.write_bytes(b"x");
        out.close().expect("closed");
        files.push(name);
    }
    let st = store();
    let first = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("first");
    assert!(first.manifest.len() > 256 << 10, "{}", first.manifest.len());
    // A fresh publisher reads the register, which holds more than 256 KiB.
    let again = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 2)
        .expect("again");
    assert!(again.blobs.is_empty(), "nothing new to upload");
    assert_eq!(again.generation, first.generation + 1);
}
