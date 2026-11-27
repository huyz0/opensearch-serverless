//! The commit reference source: a pack is named when the latest commit of
//! a shard that has a block in it records a file at the pack, and anything
//! unreadable is an error, never "not named".

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{
    BucketManifest, BucketMember, FileLocation, ManifestEntry, PlannedBlock, ShardRef,
    bucket_manifest_key, descriptor_key, encode_manifest, encode_node_pack, node_pack_key,
};
use oss_writer::pack::{CommitReferences, PackError, PackId, PackReferences};

const NODE: &str = "node-1";
const EPOCH: u64 = 3;
const ORDINAL: u64 = 5;

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn put(e: &Emulator, key: &str, bytes: &[u8]) {
    e.put(key, &mut SliceSource::new(bytes), &mut [0; 16])
        .expect("put");
}

/// A register: generation zero, then `value`.
fn register(e: &Emulator, key: &str, value: &[u8]) {
    let mut blob = 0u64.to_be_bytes().to_vec();
    blob.extend_from_slice(value);
    put(e, key, &blob);
}

/// A pack with one block for shard 0 of `logs` (uuid `u1`), body omitted.
fn pack(e: &Emulator) {
    let planned = [PlannedBlock {
        index_uuid: "u1",
        index_name: "logs",
        shard: 0,
        name: "_0.seg",
        length: 10,
    }];
    let encoded = encode_node_pack(NODE, EPOCH, ORDINAL, &planned).expect("encode");
    let mut bytes = encoded.bytes;
    bytes.extend_from_slice(&[0; 10]);
    put(
        e,
        &node_pack_key("", NODE, EPOCH, ORDINAL).expect("key"),
        &bytes,
    );
}

fn descriptor(e: &Emulator, uuid: &str, bucket: Option<&str>) {
    let bucket = bucket.map_or(String::new(), |b| format!(",\"bucket\":\"{b}\""));
    let json = format!("{{\"name\":\"logs\",\"uuid\":\"{uuid}\",\"number_of_shards\":1{bucket}}}");
    register(
        e,
        &descriptor_key("", "logs").expect("key"),
        json.as_bytes(),
    );
}

/// A commit with one file in pack `ordinal` of this node's epoch.
fn commit(ordinal: u64) -> Vec<u8> {
    let location = FileLocation::NodePacked {
        node: NODE,
        epoch: EPOCH,
        ordinal,
        offset: 0,
        length: 10,
    };
    encode_manifest(
        1,
        Some(NODE),
        &[ManifestEntry {
            name: "_0.cfs",
            location: location.encode(),
            length: 10,
        }],
        -1,
        -1,
    )
    .expect("manifest")
}

fn shard_register(e: &Emulator, commit: &[u8]) {
    let key = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    }
    .manifest_key("")
    .expect("key");
    register(e, &key, commit);
}

fn source(e: &Arc<Emulator>) -> CommitReferences {
    CommitReferences::new(Arc::clone(e) as Arc<dyn BlobStore>, "").expect("source")
}

fn asks(source: &CommitReferences) -> Result<bool, PackError> {
    source.names(&PackId {
        node: NODE,
        epoch: EPOCH,
        ordinal: ORDINAL,
    })
}

#[test]
fn a_commit_that_records_a_file_in_the_pack_names_it() {
    let e = emulator();
    pack(&e);
    descriptor(&e, "u1", None);
    shard_register(&e, &commit(ORDINAL));
    assert!(asks(&source(&e)).expect("names"));
}

#[test]
fn a_commit_that_records_another_pack_does_not() {
    let e = emulator();
    pack(&e);
    descriptor(&e, "u1", None);
    shard_register(&e, &commit(ORDINAL + 1));
    assert!(!asks(&source(&e)).expect("names"));
}

#[test]
fn a_pack_that_is_gone_is_named_by_nothing() {
    let e = emulator();
    assert!(!asks(&source(&e)).expect("names"));
}

#[test]
fn a_deleted_missing_or_recreated_index_names_nothing() {
    let e = emulator();
    pack(&e);
    shard_register(&e, &commit(ORDINAL));
    // No descriptor at all.
    assert!(!asks(&source(&e)).expect("absent"));
    // A tombstone.
    register(
        &e,
        &descriptor_key("", "logs").expect("key"),
        b"{\"tombstone\":true}",
    );
    assert!(!asks(&source(&e)).expect("tombstone"));
    // The name was reused by an index with another uuid.
    descriptor(&e, "u2", None);
    assert!(!asks(&source(&e)).expect("recreated"));
}

#[test]
fn a_shard_that_never_committed_names_nothing() {
    let e = emulator();
    pack(&e);
    descriptor(&e, "u1", None);
    assert!(!asks(&source(&e)).expect("no register"));
}

#[test]
fn a_bucketed_index_is_read_from_its_bucket_manifest() {
    let e = emulator();
    pack(&e);
    descriptor(&e, "u1", Some("b1"));
    let member = BucketMember::first("logs", Some(commit(ORDINAL))).expect("member");
    let doc = BucketManifest::empty("b1")
        .expect("empty")
        .with_member("u1", member)
        .expect("member")
        .encode()
        .expect("encode");
    register(&e, &bucket_manifest_key("", "b1").expect("key"), &doc);
    assert!(asks(&source(&e)).expect("names"));

    let member = BucketMember::first("logs", None).expect("member");
    let doc = BucketManifest::empty("b1")
        .expect("empty")
        .with_member("u1", member)
        .expect("member")
        .encode()
        .expect("encode");
    register(&e, &bucket_manifest_key("", "b1").expect("key"), &doc);
    assert!(!asks(&source(&e)).expect("no commit yet"));
}

#[test]
fn what_cannot_be_read_is_an_error_not_an_answer() {
    let e = emulator();
    pack(&e);
    descriptor(&e, "u1", None);
    shard_register(&e, b"not a manifest");
    assert!(asks(&source(&e)).is_err());

    let e = emulator();
    put(
        &e,
        &node_pack_key("", NODE, EPOCH, ORDINAL).expect("key"),
        b"garbage that is no pack header",
    );
    assert!(asks(&source(&e)).is_err());

    let e = emulator();
    pack(&e);
    register(&e, &descriptor_key("", "logs").expect("key"), b"{broken");
    assert!(asks(&source(&e)).is_err());
}

/// One pack header range read pair, then per shard a descriptor and a
/// commit register: no LIST.
#[test]
fn a_question_costs_two_reads_of_the_pack_and_two_of_registers() {
    let e = emulator();
    pack(&e);
    descriptor(&e, "u1", None);
    shard_register(&e, &commit(ORDINAL));
    let (whole, ranged) = (e.meter().count(S3Op::Get), e.meter().count(S3Op::GetRange));
    assert!(asks(&source(&e)).expect("names"));
    assert_eq!(e.meter().count(S3Op::Get) - whole, 2);
    assert_eq!(e.meter().count(S3Op::GetRange) - ranged, 2);
    assert_eq!(e.meter().count(S3Op::List), 0);
}
