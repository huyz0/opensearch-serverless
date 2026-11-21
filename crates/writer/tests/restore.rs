//! docs/specs/writer-restore: a shard's published commit is restored into a
//! writer's directory.

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::{ByteBuffersDirectory, Directory};
use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{CommitManifest, ShardRef};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;
use oss_writer::takeover::{RestoreError, restore};

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

fn mapping() -> Mapping {
    Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping")
}

fn shard(dir: &ByteBuffersDirectory, term: i64) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    Shard::new(dir, w, mapping(), term, Box::new(|| 0)).expect("shard")
}

fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

/// A shard with `ids` committed in one commit each, published.
fn published(st: &Arc<Emulator>, ids: &[&str]) -> (ByteBuffersDirectory, Vec<String>) {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, 3);
    let mut p =
        ShardPublisher::new(Arc::clone(st) as Arc<dyn BlobStore>, "", &SHARD).expect("publisher");
    let mut files = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        s.index(id, br#"{"n":1}"#, false, &Expect::default())
            .expect("indexed");
        s.refresh().expect("refresh");
        files = s.committed_files().expect("files");
        p.publish(
            &dir,
            &files,
            3,
            "node-1",
            0,
            u64::try_from(i).unwrap_or(0).saturating_add(1),
        )
        .expect("published");
    }
    drop(s);
    (dir, files)
}

fn buf() -> Vec<u8> {
    vec![0u8; 4096]
}

fn bytes(dir: &ByteBuffersDirectory, name: &str) -> Vec<u8> {
    dir.open(name).expect("opens").to_vec()
}

#[test]
fn a_shard_opened_over_the_restored_commit_holds_its_documents_and_carries_on() {
    let st = store();
    let (_, _) = published(&st, &["a", "b", "c"]);
    let dir = ByteBuffersDirectory::new();
    let restored = restore(&*st, "", &SHARD, &dir, &mut buf())
        .expect("restores")
        .expect("a commit");
    assert_eq!(restored.term, 3);
    let mut s = shard(&dir, 4);
    assert_eq!(s.max_seq_no(), 2);
    let next = s
        .index("d", br#"{"n":1}"#, false, &Expect::default())
        .expect("indexed");
    assert_eq!(next.seq_no, 3);
}

#[test]
fn every_file_of_the_commit_is_restored_with_its_bytes() {
    let st = store();
    let (source, files) = published(&st, &["a", "b"]);
    let dir = ByteBuffersDirectory::new();
    restore(&*st, "", &SHARD, &dir, &mut buf()).expect("restores");
    assert!(!files.is_empty());
    for name in &files {
        assert_eq!(bytes(&dir, name), bytes(&source, name), "{name}");
    }
}

#[test]
fn a_restore_reads_the_register_once_and_each_blob_once() {
    let st = store();
    let _ = published(&st, &["a", "b", "c"]);
    let before = (
        st.meter().count(S3Op::Get) + st.meter().count(S3Op::GetRange),
        st.meter().count(S3Op::Put),
        st.meter().count(S3Op::List),
    );
    let dir = ByteBuffersDirectory::new();
    restore(&*st, "", &SHARD, &dir, &mut buf()).expect("restores");
    let gets = st.meter().count(S3Op::Get) + st.meter().count(S3Op::GetRange) - before.0;
    // Three segments, so three blobs, and the register read.
    assert_eq!(gets, 4, "the register and a GET a blob");
    assert_eq!(st.meter().count(S3Op::Put), before.1);
    assert_eq!(st.meter().count(S3Op::List), before.2);
}

#[test]
fn a_shard_with_no_register_restores_nothing_and_reads_no_blob() {
    let st = store();
    let dir = ByteBuffersDirectory::new();
    let got = restore(&*st, "", &SHARD, &dir, &mut buf()).expect("restores");
    assert!(got.is_none());
    assert!(dir.list_all().expect("lists").is_empty());
    assert_eq!(
        st.meter().count(S3Op::Get) + st.meter().count(S3Op::GetRange),
        1,
        "the register read alone"
    );
}

#[test]
fn a_blob_that_ends_before_its_files_do_is_refused() {
    let st = store();
    let _ = published(&st, &["a"]);
    let key = SHARD.manifest_key("").expect("key");
    let mut b = vec![0u8; 1 << 16];
    let read = st
        .read_register(&key, &mut b)
        .expect("reads")
        .expect("registered");
    let manifest = CommitManifest::parse(&b[..read.len]).expect("parses");
    let extents = SHARD.resolve("", &manifest).expect("resolves");
    let packed = extents
        .iter()
        .find(|e| e.offset > 0 || e.name.ends_with(".si"))
        .or(extents.first())
        .expect("a file");
    let short: &[u8] = b"x";
    st.put(&packed.key, &mut SliceSource::new(short), &mut [0u8; 16])
        .expect("overwrites");
    let dir = ByteBuffersDirectory::new();
    let got = restore(&*st, "", &SHARD, &dir, &mut buf());
    assert!(matches!(got, Err(RestoreError::Short(_))), "{got:?}");
}
