//! docs/specs/bucket-member-register/SPEC.md: a bucket member's publisher
//! records its commit in its bucket, over the in-process emulator.

// ARITH: test request counts are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{BucketManifest, BucketMember, CommitManifest, ShardRef};
use oss_writer::bucket::{BucketCommits, BucketStore, ManifestWrite, MemberRegister};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::{PublishError, ShardPublisher};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const BUCKET: &str = "s02-1";

const MEMBER: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "m1",
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

struct Rig {
    store: Arc<Emulator>,
    commits: Arc<BucketCommits>,
}

impl Rig {
    /// A bucket with members `m1` (no commit yet) and `m2`.
    fn new() -> Self {
        let store = Arc::new(Emulator::new(1 << 30, 7));
        let dynstore = || Arc::clone(&store) as Arc<dyn BlobStore>;
        let bucket = BucketStore::new(dynstore(), "", ManifestWrite::Json);
        let mut manifest = BucketManifest::empty(BUCKET).unwrap().at_term(4).unwrap();
        for (uuid, name) in [("m1", "logs"), ("m2", "other")] {
            manifest = manifest
                .with_member(uuid, BucketMember::first(name, None).unwrap())
                .unwrap();
        }
        assert!(bucket.create(&manifest).unwrap());
        let commits = Arc::new(BucketCommits::new(BucketStore::new(
            dynstore(),
            "",
            ManifestWrite::Json,
        )));
        Self { store, commits }
    }

    fn publisher(&self, shard: &ShardRef<'_>, bucket: &str) -> ShardPublisher {
        let mut p = ShardPublisher::new(Arc::clone(&self.store) as Arc<dyn BlobStore>, "", shard)
            .expect("publisher");
        let register = MemberRegister::new(Arc::clone(&self.commits), bucket, shard.uuid);
        p.commit_register(Arc::new(register));
        p
    }

    fn count(&self, op: S3Op) -> u64 {
        self.store.meter().count(op)
    }

    fn reads(&self) -> u64 {
        self.count(S3Op::Get) + self.count(S3Op::GetRange) + self.count(S3Op::Head)
    }

    fn entry(&self, uuid: &str) -> (u64, Option<Vec<u8>>) {
        let m = self.commits.member_commit(BUCKET, uuid).unwrap();
        m.map_or((0, None), |m| (m.version, Some(m.commit)))
    }
}

#[test]
fn a_default_register_publish_is_one_swap() {
    let st = Arc::new(Emulator::new(1 << 30, 7));
    let mut p = ShardPublisher::new(Arc::clone(&st) as Arc<dyn BlobStore>, "", &MEMBER).unwrap();
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let files = commit(&mut s, "a");
    p.publish(&dir, &files, 3, "node-1", 0, 1).unwrap();
    let puts = st.meter().count(S3Op::Put);
    let files = commit(&mut s, "b");
    p.publish(&dir, &files, 3, "node-1", 1, 2).unwrap();
    assert_eq!(st.meter().count(S3Op::Put), puts + 2, "a blob and a swap");
}

#[test]
fn a_member_publish_lands_in_its_bucket_entry() {
    let rig = Rig::new();
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let mut p = rig.publisher(&MEMBER, BUCKET);
    let files = commit(&mut s, "a");
    let published = p.publish(&dir, &files, 3, "node-1", 0, 1).unwrap();
    let (version, commit_bytes) = rig.entry("m1");
    assert_eq!(version, 1);
    assert_eq!(commit_bytes.as_deref(), Some(published.manifest.as_slice()));
    assert_eq!(published.generation, 1);
    assert_eq!(CommitManifest::parse(&published.manifest).unwrap().term, 3);
    // the shard's own register was never written
    let own = shard_register_key(&MEMBER);
    assert!(
        rig.store
            .read_register(&own, &mut vec![0; 4096])
            .unwrap()
            .is_none()
    );
    assert_eq!(rig.count(S3Op::List), 0);
}

fn shard_register_key(shard: &ShardRef<'_>) -> String {
    shard.manifest_key("").unwrap()
}

#[test]
fn a_members_second_publish_reads_nothing() {
    let rig = Rig::new();
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let mut p = rig.publisher(&MEMBER, BUCKET);
    let files = commit(&mut s, "a");
    p.publish(&dir, &files, 3, "node-1", 0, 1).unwrap();
    let puts = rig.count(S3Op::Put);
    let reads = rig.reads();
    let files = commit(&mut s, "b");
    p.publish(&dir, &files, 3, "node-1", 1, 2).unwrap();
    assert_eq!(
        rig.count(S3Op::Put) - puts,
        2,
        "one blob, one manifest swap"
    );
    assert_eq!(rig.reads(), reads, "no reads");
    assert_eq!(rig.entry("m1").0, 2);
}

#[test]
fn a_siblings_publish_does_not_fail_this_members() {
    let rig = Rig::new();
    let dir1 = ByteBuffersDirectory::new();
    let dir2 = ByteBuffersDirectory::new();
    let (mut s1, mut s2) = (shard(&dir1), shard(&dir2));
    let sibling = ShardRef {
        index: "other",
        uuid: "m2",
        shard: 0,
    };
    let mut p1 = rig.publisher(&MEMBER, BUCKET);
    let mut p2 = rig.publisher(&sibling, BUCKET);
    let f1 = commit(&mut s1, "a");
    p1.publish(&dir1, &f1, 3, "node-1", 0, 1).unwrap();
    let f2 = commit(&mut s2, "x");
    let sib = p2.publish(&dir2, &f2, 3, "node-1", 0, 1).unwrap();
    // m1 publishes again over the version its own entry is at
    let f1 = commit(&mut s1, "b");
    p1.publish(&dir1, &f1, 3, "node-1", 1, 2).unwrap();
    assert_eq!(rig.entry("m1").0, 2);
    let (version, bytes) = rig.entry("m2");
    assert_eq!(version, 1);
    assert_eq!(bytes.as_deref(), Some(sib.manifest.as_slice()));
}

#[test]
fn a_member_builds_on_the_commit_already_in_its_entry() {
    let rig = Rig::new();
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let files = commit(&mut s, "a");
    rig.publisher(&MEMBER, BUCKET)
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .unwrap();
    // a new publisher for the same member (a restart) reads the entry, and
    // uploads only what the entry does not name
    let mut again = rig.publisher(&MEMBER, BUCKET);
    let puts = rig.count(S3Op::Put);
    let published = again.publish(&dir, &files, 3, "node-1", 0, 2).unwrap();
    assert!(published.blobs.is_empty(), "nothing new to upload");
    assert_eq!(rig.count(S3Op::Put) - puts, 1, "only the manifest swap");
    assert_eq!(rig.entry("m1").0, 2);
}

#[test]
fn a_member_publish_is_fenced_by_term_and_writer() {
    let rig = Rig::new();
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let files = commit(&mut s, "a");
    rig.publisher(&MEMBER, BUCKET)
        .publish(&dir, &files, 5, "node-1", 0, 1)
        .unwrap();
    let stale = rig
        .publisher(&MEMBER, BUCKET)
        .publish(&dir, &files, 4, "node-1", 0, 2);
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
    let foreign = rig
        .publisher(&MEMBER, BUCKET)
        .publish(&dir, &files, 5, "node-2", 0, 3);
    assert!(
        matches!(foreign, Err(PublishError::Foreign { .. })),
        "{foreign:?}"
    );
}

#[test]
fn a_non_member_or_missing_bucket_fails_with_a_bucket_error() {
    let rig = Rig::new();
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    let files = commit(&mut s, "a");
    let stranger = ShardRef {
        index: "z",
        uuid: "nobody",
        shard: 0,
    };
    let r = rig
        .publisher(&stranger, BUCKET)
        .publish(&dir, &files, 3, "node-1", 0, 1);
    assert!(matches!(r, Err(PublishError::Bucket(_))), "{r:?}");
    let r = rig
        .publisher(&MEMBER, "no-such-bucket")
        .publish(&dir, &files, 3, "node-1", 0, 1);
    assert!(matches!(r, Err(PublishError::Bucket(_))), "{r:?}");
}
