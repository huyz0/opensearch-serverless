//! A shard applying a predecessor's records: their own identities kept,
//! stale ones ignored, unsequenced ones numbered after what is held.

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_format::WalRecord;
use oss_writer::index::{Expect, NO_OPS_PERFORMED, Refused, Shard};
use oss_writer::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn shard(dir: &ByteBuffersDirectory) -> Shard<'_> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    Shard::new(dir, w, mapping, 5, Box::new(|| 0)).expect("shard")
}

fn index(id: &str, n: i64, seq: i64, term: i64, version: i64) -> WalRecord<'static> {
    WalRecord::index(id.to_owned(), format!("{{\"n\":{n}}}"), seq, term, version)
}

fn create(s: &mut Shard<'_>, id: &str) -> Result<(), Refused> {
    s.index(id, br#"{"n":0}"#, true, &Expect::default())
        .map(|_| ())
}

#[test]
fn a_replayed_record_is_a_live_document_with_its_own_identity_and_the_next_write_follows_it() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    assert!(s.replay(&index("a", 1, 7, 2, 4)).expect("replays"));
    assert_eq!(s.max_seq_no(), 7, "the highest replayed");
    assert!(
        matches!(create(&mut s, "a"), Err(Refused::Conflict(_))),
        "a is live: a create conflicts"
    );
    let next = s
        .index("b", br#"{"n":2}"#, false, &Expect::default())
        .expect("writes");
    assert_eq!((next.seq_no, next.primary_term), (8, 5));
    // The version the record carried is the one an update builds on.
    let updated = s
        .index("a", br#"{"n":3}"#, false, &Expect::default())
        .expect("updates");
    assert_eq!(updated.version, 5);
}

#[test]
fn a_replayed_delete_leaves_a_tombstone_a_create_may_pass() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    s.replay(&index("a", 1, 0, 2, 1)).expect("index");
    s.replay(&WalRecord::deletion("a".to_owned(), 1, 2, 2))
        .expect("delete");
    assert_eq!(s.max_seq_no(), 1);
    create(&mut s, "a").expect("a is deleted: a create goes ahead");
}

#[test]
fn a_stale_record_changes_nothing_and_an_unsequenced_one_takes_the_next_number() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    s.replay(&index("a", 1, 3, 2, 1)).expect("first");
    // Older than what `a` holds: ignored, even though it deletes.
    assert!(
        !s.replay(&WalRecord::deletion("a".to_owned(), 2, 2, 9))
            .expect("stale"),
        "reported as not applied"
    );
    assert!(
        matches!(create(&mut s, "a"), Err(Refused::Conflict(_))),
        "a is still live"
    );
    assert_eq!(s.max_seq_no(), 3, "a refusal takes no number");
    s.replay(&WalRecord::index_unsequenced(
        "c".to_owned(),
        r#"{"n":9}"#.to_owned(),
    ))
    .expect("unsequenced");
    assert_eq!(s.max_seq_no(), 4, "numbered after what is held");
    assert_eq!(NO_OPS_PERFORMED, -1);
}

#[test]
fn a_replayed_update_replaces_the_document_a_segment_holds() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    s.replay(&index("a", 1, 0, 2, 1)).expect("first");
    s.replay(&index("a", 2, 1, 2, 2)).expect("update");
    s.refresh().expect("commits");
    // Only a segment knows `a` now: it finds the newer document alone.
    let expect = Expect {
        if_seq_no: 1,
        if_primary_term: 2,
        ..Expect::default()
    };
    let written = s
        .index("a", br#"{"n":3}"#, false, &expect)
        .expect("builds on the replayed update, not the replaced document");
    assert_eq!(written.version, 3);
}

#[test]
fn a_replayed_delete_removes_the_document_a_segment_holds() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    s.replay(&index("a", 1, 0, 2, 1)).expect("index");
    s.refresh().expect("commits");
    s.replay(&WalRecord::deletion("a".to_owned(), 1, 2, 2))
        .expect("delete");
    s.refresh().expect("commits");
    // Only segments know `a` now, and the delete soft-deleted its document.
    create(&mut s, "a").expect("a is deleted: a create goes ahead");
}

/// The live documents a commit holds, the way the reader counts them.
fn live_docs(dir: &ByteBuffersDirectory) -> i32 {
    DirectoryReader::open(dir)
        .expect("opens")
        .soft_deletes_wrapper("__soft_deletes")
        .expect("wraps")
        .num_docs()
}

#[test]
fn replayed_updates_and_deletes_leave_one_live_document_an_id_and_none_after_a_delete() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir);
    s.replay(&index("a", 1, 0, 2, 1)).expect("index");
    s.replay(&index("b", 1, 1, 2, 1)).expect("index");
    s.refresh().expect("commits");
    s.replay(&index("a", 2, 2, 2, 2)).expect("update");
    s.refresh().expect("commits");
    assert_eq!(live_docs(&dir), 2, "the update replaced a's document");
    s.replay(&WalRecord::deletion("b".to_owned(), 3, 2, 2))
        .expect("delete");
    s.refresh().expect("commits");
    assert_eq!(live_docs(&dir), 1, "the delete removed b's");
}
