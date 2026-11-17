//! Publishing a shard's commits over the in-process emulator: the requests a
//! publish makes, which files it uploads, the blob names, and the fence.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{CommitManifest, ManifestEntry, ShardRef, encode_manifest};
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

fn manifest_of(store: &Emulator) -> Vec<u8> {
    let mut buf = vec![0u8; 1 << 20];
    let read = store
        .read_register("segments/logs#u1#0/manifest", &mut buf)
        .expect("reads")
        .expect("present");
    buf.get(..read.len).expect("within").to_vec()
}

#[test]
fn the_manifest_names_every_file_and_no_stage() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let files = commit(&mut s, "a");
    let out = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
    let m = CommitManifest::parse(&out.manifest).expect("parses");
    let mut named: Vec<&str> = m.files().iter().map(|f| f.name.as_ref()).collect();
    named.sort_unstable();
    let mut want: Vec<&str> = files.iter().map(String::as_str).collect();
    want.sort_unstable();
    assert_eq!(named, want);
    assert_eq!(m.stage_sequence, -1);
    assert_eq!(manifest_of(&st), out.manifest);
}

#[test]
fn the_commit_points_ride_in_the_last_segments_blob() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    commit(&mut s, "a");
    let files = commit(&mut s, "b");
    // Two segments from the writer's refreshes (or one merged): either way
    // the segments file shares a blob with a segment's files.
    let out = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
    let m = CommitManifest::parse(&out.manifest).expect("parses");
    let points = m
        .files()
        .iter()
        .find(|f| f.name.starts_with("segments_"))
        .expect("a segments file");
    let place = points.location.encode();
    let shares = m
        .files()
        .iter()
        .filter(|f| !f.name.starts_with("segments_"))
        .any(|f| {
            let there = f.location.encode();
            there.split('#').next() == place.split('#').next()
        });
    assert!(shares, "{place}");
}

#[test]
fn a_new_file_of_a_published_segment_takes_a_fresh_blob_name_without_trying_the_plain_one() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st);
    let files = commit(&mut s, "a");
    p.publish(&dir, &files, 3, "node-1", 0, 1).expect("first");
    // A replacement soft-deletes the first document: its segment gains a file.
    s.index("a", br#"{"n":2}"#, false, &Expect::default())
        .expect("replaced");
    s.refresh().expect("refresh");
    let files = s.committed_files().expect("files");
    let puts = count(&st, S3Op::Put);
    let out = p.publish(&dir, &files, 3, "node-1", 1, 2).expect("second");
    let m = CommitManifest::parse(&out.manifest).expect("parses");
    assert!(m.files().len() > 2);
    assert_eq!(
        count(&st, S3Op::Put) - puts,
        out.blobs.len() as u64 + 1,
        "a PUT a blob and the swap, no create tried and refused"
    );
}

#[test]
fn an_empty_file_is_a_blob_of_its_own() {
    use lucene_store::{DataOutput, Directory};
    let dir = ByteBuffersDirectory::new();
    let out = dir.create_output("_0.empty").expect("output");
    out.close().expect("closed");
    let mut full = dir.create_output("_0.dat").expect("output");
    full.write_bytes(b"x");
    full.close().expect("closed");
    let st = store();
    let files = vec!["_0.empty".to_owned(), "_0.dat".to_owned()];
    let out = publisher(&st)
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
    let m = CommitManifest::parse(&out.manifest).expect("parses");
    let empty = m.file("_0.empty").expect("named");
    assert_eq!(empty.length, Some(0));
    assert_eq!(empty.location.encode(), "t=3");
    let key = "segments/logs#u1#0/t=3/_0.empty";
    assert!(
        st.head(key).expect("head").is_some(),
        "the empty blob exists"
    );
}

#[test]
fn a_manifest_too_large_to_read_is_refused_rather_than_retried() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let files = commit(&mut s, "a");
    st.create_register("segments/logs#u1#0/manifest", &vec![b' '; (4 << 20) + 1])
        .expect("created");
    let r = publisher(&st).publish(&dir, &files, 3, "node-1", 0, 1);
    assert!(
        matches!(
            r,
            Err(PublishError::Unusable("a manifest too large to read"))
        ),
        "{r:?}"
    );
}

#[test]
fn a_publisher_names_its_shard_and_whether_it_remembers_a_commit() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let st = store();
    let mut p = publisher(&st);
    let before = format!("{p:?}");
    assert!(before.contains("remembered: false"), "{before}");
    assert!(before.contains("segments/logs#u1#0/"), "{before}");
    let files = commit(&mut s, "a");
    p.publish(&dir, &files, 3, "node-1", 0, 1).expect("first");
    assert!(format!("{p:?}").contains("remembered: true"));
}

/// A publisher over a store whose first swap is preceded by `rival`'s
/// publish of `rival_term` as `rival_writer`, so the swap loses.
fn lose_the_first_swap(
    remembered: bool,
    rival_term: u64,
    rival_writer: &'static str,
) -> (
    Result<oss_writer::publish::Published, PublishError>,
    Vec<u8>,
) {
    let dir = Arc::new(ByteBuffersDirectory::new());
    let mut s = shard(&dir);
    let st = store();
    let files = commit(&mut s, "a");
    let mut seeded = publisher(&st);
    seeded
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("seeded");
    let files2 = commit(&mut s, "b");
    let armed = Arc::new(AtomicBool::new(false));
    let trip = Arc::clone(&armed);
    let hooked = common::Hooked::swapping(&st, move |e, key, expected, value| {
        if trip.swap(false, Ordering::SeqCst) {
            // The rival moves the register on before this swap lands.
            let entry = ManifestEntry {
                name: "_9.cfs",
                location: "t=1".to_owned(),
                length: 0,
            };
            let theirs =
                encode_manifest(rival_term, Some(rival_writer), &[entry], 1, -1).expect("encodes");
            e.swap_register(key, expected, &theirs)
                .expect("rival swaps");
        }
        e.swap_register(key, expected, value)
    });
    let mut p = ShardPublisher::new(hooked, "", &SHARD).expect("publisher");
    if remembered {
        p.publish(&*dir, &files, 3, "node-1", 0, 2)
            .expect("remembers");
    }
    armed.store(true, Ordering::SeqCst);
    let out = p.publish(&*dir, &files2, 3, "node-1", 1, 3);
    (out, manifest_of(&st))
}

#[test]
fn a_publisher_that_remembers_nothing_takes_the_winning_manifest_of_its_own_node() {
    let (out, register) = lose_the_first_swap(false, 3, "node-1");
    let out = out.expect("published");
    assert!(out.superseded);
    assert_eq!(out.manifest, register);
}

#[test]
fn a_lost_swap_to_another_writer_or_a_newer_term_is_a_stale_writer() {
    let (other, _) = lose_the_first_swap(false, 3, "node-2");
    assert!(
        matches!(
            other,
            Err(PublishError::Stale {
                term: 3,
                found: Some(3)
            })
        ),
        "{other:?}"
    );
    let (newer, _) = lose_the_first_swap(false, 4, "node-1");
    assert!(
        matches!(
            newer,
            Err(PublishError::Stale {
                term: 3,
                found: Some(4)
            })
        ),
        "{newer:?}"
    );
}

#[test]
fn a_publisher_that_remembers_a_commit_publishes_again_over_the_winner_once() {
    let (out, register) = lose_the_first_swap(true, 3, "node-1");
    let out = out.expect("published");
    assert!(!out.superseded);
    assert_eq!(out.manifest, register);
}
