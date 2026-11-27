//! docs/specs/writer-commit-checkpoint: the local checkpoint a published
//! commit records is read from its `segments_N` alone.

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::ShardRef;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;
use oss_writer::takeover::commit_checkpoint;

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

fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

/// Publishes a commit of `ids` and answers its sequence numbers' highest.
fn publish(st: &Arc<Emulator>, ids: &[&str]) {
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    let mut s = Shard::new(&dir, w, mapping, 3, Box::new(|| 0)).expect("shard");
    for id in ids {
        s.index(id, br#"{"n":1}"#, false, &Expect::default())
            .expect("indexed");
    }
    s.refresh().expect("refresh");
    let files = s.committed_files().expect("files");
    let mut p =
        ShardPublisher::new(Arc::clone(st) as Arc<dyn BlobStore>, "", &SHARD).expect("publisher");
    p.publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
}

#[test]
fn a_published_commit_answers_the_highest_sequence_number_it_holds() {
    let st = store();
    publish(&st, &["a", "b", "c"]);
    assert_eq!(
        commit_checkpoint(&*st, "", &SHARD).expect("read"),
        Some(Some(2))
    );
}

#[test]
fn a_shard_with_no_commit_answers_none() {
    let st = store();
    assert_eq!(commit_checkpoint(&*st, "", &SHARD).expect("read"), None);
}

#[test]
fn reading_a_checkpoint_is_a_register_read_and_one_ranged_get_and_no_list_or_put() {
    let st = store();
    publish(&st, &["a", "b"]);
    let m = st.meter();
    let before = (
        m.count(S3Op::Get) + m.count(S3Op::GetRange),
        m.count(S3Op::Put),
        m.count(S3Op::List),
    );
    commit_checkpoint(&*st, "", &SHARD).expect("read");
    let gets = m.count(S3Op::Get) + m.count(S3Op::GetRange) - before.0;
    assert_eq!(gets, 2, "the register and segments_N");
    assert_eq!(
        (m.count(S3Op::Put), m.count(S3Op::List)),
        (before.1, before.2)
    );
}
