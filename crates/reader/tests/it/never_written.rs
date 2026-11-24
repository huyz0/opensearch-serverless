//! An index that was created and never written: its descriptor exists and
//! no commit does. It opens as a shard of no documents, with no request
//! past the descriptor, the register and the shard head.

use oss_blob::{PooledStore, S3Op, SliceSource};
use oss_format::{
    BucketManifest, BucketMember, IndexRecord, ShardHead, bucket_manifest_key, descriptor_key,
};
use oss_reader::{ReaderError, ShardFiles, open_shard};

use crate::{BASE, cache, counts, since, store_with_fixtures};

const NAME: &str = "never-written";
const BUCKET: &str = "b2";

fn register(value: &[u8]) -> Vec<u8> {
    let mut blob = 1u64.to_be_bytes().to_vec();
    blob.extend_from_slice(value);
    blob
}

fn put<S: oss_blob::BlobStore>(store: &PooledStore<S>, key: &str, bytes: &[u8]) {
    store.put(key, &mut SliceSource::new(bytes)).expect("put");
}

fn describe<S: oss_blob::BlobStore>(store: &PooledStore<S>, bucket: Option<&str>) {
    let record = IndexRecord {
        bucket: bucket.map(Into::into),
        ..IndexRecord::new(NAME, "never-uuid", 1)
    };
    let mut buf = vec![0u8; 1 << 16];
    let n = record.write(&mut buf).expect("write");
    put(
        store,
        &descriptor_key(BASE, NAME).expect("key"),
        &register(&buf[..n]),
    );
}

fn assert_no_documents_and_no_list(
    store: &std::sync::Arc<PooledStore<oss_blob::Emulator>>,
    before: [u64; 7],
) {
    let shard = open_shard(store.clone(), cache(), BASE, NAME, 0).expect("opens");
    assert_eq!(shard.reader.max_doc(), 0);
    assert_eq!(shard.reader.num_docs(), 0);
    let made = since(store, before);
    for op in [S3Op::List, S3Op::Head, S3Op::Put, S3Op::Copy, S3Op::Delete] {
        assert!(made.iter().all(|(o, _)| *o != op), "{op:?}: {made:?}");
    }
    let gets: u64 = made
        .iter()
        .filter(|(o, _)| matches!(o, S3Op::Get | S3Op::GetRange))
        .map(|(_, n)| n)
        .sum();
    assert!(
        gets <= 3,
        "the descriptor, the register and the head only: {made:?}"
    );
}

#[test]
fn an_index_with_a_descriptor_and_no_commit_opens_with_no_documents() {
    let store = store_with_fixtures();
    describe(&store, None);
    let before = counts(&store);
    assert_no_documents_and_no_list(&store, before);
}

#[test]
fn a_bucketed_member_with_no_commit_yet_opens_with_no_documents() {
    let store = store_with_fixtures();
    describe(&store, Some(BUCKET));
    let member = BucketMember::first(NAME, None).expect("member");
    let manifest = BucketManifest::empty(BUCKET)
        .expect("empty")
        .with_member("never-uuid", member)
        .expect("member");
    put(
        &store,
        &bucket_manifest_key(BASE, BUCKET).expect("key"),
        &register(&manifest.encode().expect("encodes")),
    );
    let before = counts(&store);
    assert_no_documents_and_no_list(&store, before);
}

#[test]
fn the_files_of_a_never_written_shard_are_the_one_empty_commit() {
    let store = store_with_fixtures();
    describe(&store, None);
    let files = ShardFiles::open(&store, BASE, NAME, 0).expect("opens");
    assert_eq!(files.len(), 1);
}

fn head_of(uuid: &str) -> Vec<u8> {
    ShardHead {
        index: NAME.into(),
        shard: 0,
        term: 3,
        owner_node_id: None,
        owner_ephemeral_id: None,
        lease_expires_at_millis: 0,
        index_uuid: Some(uuid.into()),
        writers: Vec::new(),
    }
    .to_bytes()
    .expect("head")
}

fn put_head<S: oss_blob::BlobStore>(store: &PooledStore<S>, uuid: &str) {
    put(
        store,
        &oss_cluster::heads::shard_head_key(BASE, NAME, 0),
        &register(&head_of(uuid)),
    );
}

#[test]
fn a_shard_a_writer_took_that_lost_its_commit_register_is_an_error_not_an_empty_shard() {
    let store = store_with_fixtures();
    describe(&store, None);
    put_head(&store, "never-uuid");
    let err = ShardFiles::open(&store, BASE, NAME, 0).expect_err("lost the register");
    assert!(
        matches!(err, ReaderError::Blob(oss_blob::BlobError::NotFound)),
        "{err}"
    );
}

#[test]
fn a_head_an_earlier_index_of_the_name_left_does_not_make_the_new_one_lose_a_commit() {
    let store = store_with_fixtures();
    describe(&store, None);
    put_head(&store, "an-earlier-uuid");
    let before = counts(&store);
    assert_no_documents_and_no_list(&store, before);
}

#[test]
fn a_bucketed_index_with_no_bucket_manifest_is_an_error() {
    let store = store_with_fixtures();
    describe(&store, Some(BUCKET));
    let err = ShardFiles::open(&store, BASE, NAME, 0).expect_err("no manifest");
    assert!(
        matches!(err, ReaderError::Blob(oss_blob::BlobError::NotFound)),
        "{err}"
    );
}
