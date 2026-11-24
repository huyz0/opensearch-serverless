//! Opening a bucketed index: its commit is its entry in the bucket manifest.

use oss_blob::{ByteRange, PooledStore, S3Op, SliceSource};
use oss_format::{
    BucketManifest, BucketMember, FormatError, IndexRecord, Register, ShardRef,
    bucket_manifest_key, descriptor_key,
};
use oss_reader::{ReaderError, ShardFiles};

use crate::{BASE, INDEX, counts, since, store_with_fixtures};

const BUCKET: &str = "b1";

fn register(value: &[u8]) -> Vec<u8> {
    let mut blob = 1u64.to_be_bytes().to_vec();
    blob.extend_from_slice(value);
    blob
}

fn put<S: oss_blob::BlobStore>(store: &PooledStore<S>, key: &str, bytes: &[u8]) {
    store.put(key, &mut SliceSource::new(bytes)).expect("put");
}

/// The fixture's commit register value, as the shell wrote it.
fn commit_of<S: oss_blob::BlobStore>(store: &PooledStore<S>, uuid: &str) -> Vec<u8> {
    let at = ShardRef {
        index: INDEX,
        uuid,
        shard: 0,
    };
    let mut bytes = Vec::new();
    store
        .get(
            &at.manifest_key(BASE).expect("key"),
            ByteRange::Full,
            &mut |c: &[u8]| {
                bytes.extend_from_slice(c);
                Ok(())
            },
        )
        .expect("read");
    Register::parse(&bytes).expect("register").value.to_vec()
}

/// The fixture index re-described as a member of `BUCKET`, whose manifest
/// holds `member` for it.
fn bucketed<S: oss_blob::BlobStore>(
    store: &PooledStore<S>,
    uuid: &str,
    member: Option<BucketMember>,
) {
    let record = IndexRecord {
        bucket: Some(BUCKET.into()),
        ..IndexRecord::new(INDEX, uuid, 1)
    };
    let mut buf = vec![0u8; 1 << 16];
    let n = record.write(&mut buf).expect("write");
    put(
        store,
        &descriptor_key(BASE, INDEX).expect("key"),
        &register(&buf[..n]),
    );
    let mut manifest = BucketManifest::empty(BUCKET).expect("empty");
    if let Some(member) = member {
        manifest = manifest.with_member(uuid, member).expect("member");
    }
    put(
        store,
        &bucket_manifest_key(BASE, BUCKET).expect("key"),
        &register(&manifest.encode().expect("encodes")),
    );
}

#[test]
fn a_member_opens_to_the_files_its_entry_names_with_two_gets() {
    let store = store_with_fixtures();
    let (plain, meta) = ShardFiles::open_with_meta(&store, BASE, INDEX, 0).expect("plain");
    let commit = commit_of(&store, &meta.uuid);
    let member = BucketMember::first(INDEX, Some(commit)).expect("member");
    bucketed(&store, &meta.uuid, Some(member));
    let before = counts(&store);
    let files = ShardFiles::open(&store, BASE, INDEX, 0).expect("bucketed");
    assert_eq!(since(&store, before), [(S3Op::Get, 2)]);
    assert_eq!(files.term, plain.term);
    assert_eq!(files.len(), plain.len());
    assert_eq!(
        files.names().collect::<Vec<_>>(),
        plain.names().collect::<Vec<_>>()
    );
    assert_eq!(files.get("_0.si"), plain.get("_0.si"));
}

#[test]
fn a_bucketed_index_has_only_shard_zero() {
    let store = store_with_fixtures();
    let (_, meta) = ShardFiles::open_with_meta(&store, BASE, INDEX, 0).expect("plain");
    let commit = commit_of(&store, &meta.uuid);
    bucketed(
        &store,
        &meta.uuid,
        Some(BucketMember::first(INDEX, Some(commit)).expect("member")),
    );
    let err = ShardFiles::open(&store, BASE, INDEX, 1).expect_err("no shard 1");
    assert!(
        matches!(
            err,
            ReaderError::Format(FormatError::NotFollowable {
                reason: "no such shard",
                ..
            })
        ),
        "{err}"
    );
}

#[test]
fn a_member_without_a_commit_or_a_manifest_entry_is_not_followable() {
    let store = store_with_fixtures();
    let (_, meta) = ShardFiles::open_with_meta(&store, BASE, INDEX, 0).expect("plain");
    bucketed(
        &store,
        &meta.uuid,
        Some(BucketMember::first(INDEX, None).expect("member")),
    );
    let err = ShardFiles::open(&store, BASE, INDEX, 0).expect_err("no commit");
    assert!(err.to_string().contains("no commit"), "{err}");
    bucketed(&store, &meta.uuid, None);
    let err = ShardFiles::open(&store, BASE, INDEX, 0).expect_err("not a member");
    assert!(err.to_string().contains("not a member"), "{err}");
}
