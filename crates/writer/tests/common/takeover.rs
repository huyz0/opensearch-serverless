//! Heads, publishers and predecessors the takeover tests share.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::committer::{Rig, rec, shard};
use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::shard_head_key;
use oss_format::{CommitManifest, MAX_SHARD_HEAD_BYTES, ShardHead, ShardRef};
use oss_writer::index::Shard;
use oss_writer::log::NodeLogTakeover;
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;
use oss_writer::takeover::Predecessors;

pub const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

pub fn new_shard(dir: &ByteBuffersDirectory, term: i64) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    Shard::new(dir, w, mapping, term, Box::new(|| 0)).expect("shard")
}

pub fn head(n: u32, term: i64, writers: &[&str]) -> ShardHead {
    ShardHead {
        index: "logs".to_owned(),
        shard: n,
        term,
        owner_node_id: Some("node-2".to_owned()),
        owner_ephemeral_id: Some("e2".to_owned()),
        lease_expires_at_millis: 1_000_000,
        index_uuid: Some(format!("uuid-{n}")),
        writers: writers.iter().map(|w| (*w).to_owned()).collect(),
    }
}

pub fn put_head(r: &Rig, h: &ShardHead) {
    let key = shard_head_key("", &h.index, h.shard);
    r.store
        .create_register(&key, &h.to_bytes().expect("fits"))
        .expect("creates");
}

pub fn read_head(r: &Rig, n: u32) -> ShardHead {
    let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
    let read = r
        .store
        .read_register(&shard_head_key("", "logs", n), &mut buf)
        .expect("reads")
        .expect("exists");
    ShardHead::parse(&buf[..read.len]).expect("parses")
}

pub fn shard_ref(n: u32) -> (String, ShardRef<'static>) {
    let uuid = format!("uuid-{n}");
    let leaked: &'static str = Box::leak(uuid.clone().into_boxed_str());
    (
        uuid,
        ShardRef {
            index: "logs",
            uuid: leaked,
            shard: n,
        },
    )
}

pub fn publisher(r: &Rig, n: u32) -> ShardPublisher {
    let (_, sref) = shard_ref(n);
    ShardPublisher::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "", &sref).expect("publisher")
}

/// The predecessor's log as a successor reads it, behind a factory that
/// counts its calls.
pub fn predecessors(r: &Rig, calls: &Arc<AtomicU64>) -> Predecessors {
    let log = r.log.clone();
    let epochs = r.committer.positions();
    let calls = Arc::clone(calls);
    Predecessors::new(Box::new(move |_writer| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(NodeLogTakeover::new(
            log.clone(),
            epochs.clone(),
            "node-2",
            2,
        ))))
    }))
}

/// The fence a batch places once on each predecessor's epochs, before the
/// shards it takes are counted.
pub fn fenced(preds: &Predecessors) {
    preds
        .of("node-1")
        .expect("made")
        .expect("a log")
        .fence_all()
        .expect("fences");
}

pub fn write_three(r: &Rig, n: u32) {
    for seq in 0..3 {
        r.committer
            .commit(&shard(n), 1, &[rec(&format!("a{seq}"), seq)])
            .expect("lands");
    }
}

pub fn registered_term(r: &Rig, n: u32) -> u64 {
    let (_, sref) = shard_ref(n);
    let mut buf = vec![0u8; 1 << 20];
    let read = r
        .store
        .read_register(&sref.manifest_key("").expect("key"), &mut buf)
        .expect("reads")
        .expect("a commit is registered");
    CommitManifest::parse(&buf[..read.len])
        .expect("parses")
        .term
}

pub fn registered_wal_ordinal(r: &Rig, n: u32) -> i64 {
    let (_, sref) = shard_ref(n);
    let mut buf = vec![0u8; 1 << 20];
    let read = r
        .store
        .read_register(&sref.manifest_key("").expect("key"), &mut buf)
        .expect("reads")
        .expect("a commit is registered");
    CommitManifest::parse(&buf[..read.len])
        .expect("parses")
        .wal_ordinal
}

pub fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}
