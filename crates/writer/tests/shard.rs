//! A shard beyond the recorded steps: versions found in segments after a
//! refresh, deletes outliving a refresh until `gc_deletes`, refusals that
//! take no sequence number, and the request checks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::{
    Expect, GC_DELETES_MILLIS, MATCH_DELETED, NO_OPS_PERFORMED, Outcome, Refused, Shard,
    UNASSIGNED_SEQ_NO, VersionType, validate,
};
use oss_writer::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn shard<'d>(dir: &'d ByteBuffersDirectory, clock: &Arc<AtomicU64>) -> Shard<'d> {
    let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mapping = Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping");
    let clock = Arc::clone(clock);
    Shard::new(
        dir,
        w,
        mapping,
        3,
        Box::new(move || clock.load(Ordering::Relaxed)),
    )
    .expect("shard")
}

fn seq(if_seq_no: i64, if_primary_term: i64) -> Expect {
    Expect {
        if_seq_no,
        if_primary_term,
        ..Expect::default()
    }
}

#[test]
fn a_version_is_found_in_the_map_or_in_a_segment() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    let mut s = shard(&dir, &clock);
    assert_eq!(s.max_seq_no(), NO_OPS_PERFORMED);
    let a = s
        .index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert_eq!(
        (a.outcome, a.version, a.seq_no, a.primary_term),
        (Outcome::Created, 1, 0, 3)
    );
    s.refresh().unwrap();
    // Only the segment knows `a` now.
    let b = s.index("a", br#"{"n":2}"#, false, &seq(0, 3)).unwrap();
    assert_eq!((b.outcome, b.version, b.seq_no), (Outcome::Updated, 2, 1));
    s.refresh().unwrap();
    // The replaced document is soft-deleted: the newer one is found.
    let Err(Refused::Conflict(m)) = s.index("a", br#"{"n":3}"#, false, &seq(0, 3)) else {
        panic!("expected a conflict");
    };
    assert_eq!(
        m,
        "[a]: version conflict, required seqNo [0], primary term [3]. current document has seqNo [1] and primary term [3]"
    );
    let d = s.delete("a", &Expect::default()).unwrap();
    assert_eq!((d.outcome, d.version, d.seq_no), (Outcome::Deleted, 3, 2));
}

#[test]
fn a_delete_outlives_a_refresh_until_gc_deletes() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(1_000));
    let mut s = shard(&dir, &clock);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    s.delete("a", &Expect::default()).unwrap();
    s.refresh().unwrap();
    // The tombstone's version continues.
    let again = s
        .index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert_eq!((again.outcome, again.version), (Outcome::Created, 3));
    s.delete("a", &Expect::default()).unwrap();
    // Exactly gc_deletes old, the delete still counts.
    clock.store(1_000 + GC_DELETES_MILLIS, Ordering::Relaxed);
    s.refresh().unwrap();
    let external = Expect {
        version: 4,
        version_type: VersionType::External,
        ..Expect::default()
    };
    assert!(matches!(
        s.index("a", br#"{"n":1}"#, false, &external),
        Err(Refused::Conflict(_))
    ));
    clock.store(1_001 + GC_DELETES_MILLIS, Ordering::Relaxed);
    s.refresh().unwrap();
    // Pruned: the id is new again, as the Java's is once gc_deletes passed.
    let fresh = s
        .index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert_eq!((fresh.outcome, fresh.version), (Outcome::Created, 1));
}

#[test]
fn a_refused_write_takes_no_sequence_number_and_changes_nothing() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    let mut s = shard(&dir, &clock);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    let create = s.index("a", br#"{"n":2}"#, true, &Expect::default());
    assert_eq!(
        create,
        Err(Refused::Conflict(
            "[a]: version conflict, document already exists (current version [1])".into()
        ))
    );
    let Err(Refused::Mapping(e)) = s.index("b", br#"{"n":"x"}"#, false, &Expect::default()) else {
        panic!("expected a mapping refusal");
    };
    assert_eq!(e.ty, "mapper_parsing_exception");
    let Err(Refused::Mapping(e)) = s.index("", br#"{"n":1}"#, false, &Expect::default()) else {
        panic!("expected an id refusal");
    };
    assert_eq!(e.reason, "Ids can't be empty");
    let external = Expect {
        version: 1,
        version_type: VersionType::External,
        ..Expect::default()
    };
    assert!(matches!(
        s.delete("a", &external),
        Err(Refused::Conflict(_))
    ));
    assert_eq!(s.max_seq_no(), 0);
    let next = s
        .index("a", br#"{"n":3}"#, false, &Expect::default())
        .unwrap();
    assert_eq!((next.version, next.seq_no), (2, 1));
    s.refresh().unwrap();
    let missing = s.delete("b", &Expect::default()).unwrap();
    assert_eq!(
        (missing.outcome, missing.version, missing.seq_no),
        (Outcome::NotFound, 1, 2)
    );
}

#[test]
fn a_dynamic_field_grows_the_shards_mapping() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    let mut s = shard(&dir, &clock);
    s.index("a", br#"{"t":"x"}"#, false, &Expect::default())
        .unwrap();
    assert!(s.mapping().to_json().contains(r#""t":{"type":"text""#));
}

#[test]
fn requests_are_checked_as_the_java_validates_them() {
    let ext = |version| Expect {
        version,
        version_type: VersionType::External,
        ..Expect::default()
    };
    assert_eq!(validate(&Expect::default(), false), Ok(()));
    assert_eq!(validate(&Expect::default(), true), Ok(()));
    assert_eq!(validate(&ext(0), false), Ok(()));
    assert_eq!(
        validate(&ext(-5), false),
        Err("Validation Failed: 1: illegal version value [-5] for version type [EXTERNAL];".into())
    );
    let internal = Expect {
        version: 0,
        ..Expect::default()
    };
    assert_eq!(
        validate(&internal, false),
        Err("Validation Failed: 1: illegal version value [0] for version type [INTERNAL];2: internal versioning can not be used for optimistic concurrency control. Please use `if_seq_no` and `if_primary_term` instead;".into())
    );
    let gte = Expect {
        if_seq_no: 1,
        if_primary_term: 1,
        ..ext(2)
    };
    assert_eq!(
        validate(
            &Expect {
                version_type: VersionType::ExternalGte,
                ..gte
            },
            false
        ),
        Err("Validation Failed: 1: compare and write operations can not use versioning;".into())
    );
    assert_eq!(
        validate(&seq(1, 0), false),
        Err("Validation Failed: 1: ifSeqNo is set, but primary term is [0];".into())
    );
    assert_eq!(
        validate(&seq(UNASSIGNED_SEQ_NO, 4), false),
        Err("Validation Failed: 1: ifSeqNo is unassigned, but primary term is [4];".into())
    );
    assert_eq!(
        validate(&ext(1), true),
        Err("Validation Failed: 1: create operations only support internal versioning. use index instead;".into())
    );
    let explicit = Expect {
        version: 2,
        ..Expect::default()
    };
    assert_eq!(
        validate(&explicit, true),
        Err("Validation Failed: 1: create operations do not support explicit versions. use index instead;".into())
    );
    let deleted = Expect {
        version: MATCH_DELETED,
        ..Expect::default()
    };
    assert_eq!(validate(&deleted, true), Ok(()));
    assert_eq!(validate(&deleted, false), Ok(()));
    assert_eq!(
        validate(&seq(1, 1), true),
        Err("Validation Failed: 1: create operations do not support compare and set. use index instead;".into())
    );
    assert_eq!(validate(&seq(1, 1), false), Ok(()));
    // An internal explicit version beside a compare-and-set is refused too.
    let both = Expect {
        version: 2,
        ..seq(1, 1)
    };
    let message = validate(&both, false).unwrap_err();
    assert!(
        message.contains("compare and write operations can not use versioning"),
        "{message}"
    );
    // Either half of the compare-and-set pair is enough to refuse a create.
    for half in [seq(1, 0), seq(UNASSIGNED_SEQ_NO, 4)] {
        let message = validate(&half, true).unwrap_err();
        assert!(
            message.contains("create operations do not support compare and set"),
            "{message}"
        );
    }
}

#[test]
fn an_expired_delete_does_not_count_even_before_a_refresh() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    let mut s = shard(&dir, &clock);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    s.delete("a", &Expect::default()).unwrap();
    clock.store(GC_DELETES_MILLIS + 1, Ordering::Relaxed);
    let again = s
        .index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert_eq!((again.outcome, again.version), (Outcome::Created, 1));
}

#[test]
fn a_reopened_shard_continues_its_sequence_numbers() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    {
        let mut s = shard(&dir, &clock);
        for id in ["a", "b", "c"] {
            s.index(id, br#"{"n":1}"#, false, &Expect::default())
                .unwrap();
        }
        s.refresh().unwrap();
    }
    let mut s = shard(&dir, &clock);
    assert_eq!(s.max_seq_no(), 2);
    let d = s
        .index("d", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert_eq!(d.seq_no, 3);
    // The committed documents' versions are found in the segments.
    let a = s.index("a", br#"{"n":2}"#, false, &seq(0, 3)).unwrap();
    assert_eq!((a.outcome, a.version, a.seq_no), (Outcome::Updated, 2, 4));
}

#[test]
fn the_version_map_counts_toward_the_indexing_buffer() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    let mut s = shard(&dir, &clock);
    assert_eq!(s.version_map_bytes(), 0);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    let one = s.version_map_bytes();
    assert!(one > 0 && s.indexing_buffer_bytes() > one);
    s.index("a", br#"{"n":2}"#, false, &Expect::default())
        .unwrap();
    assert_eq!(s.version_map_bytes(), one);
    s.index("bb", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert!(s.version_map_bytes() > 2 * one);
    s.refresh().unwrap();
    assert_eq!(s.version_map_bytes(), 0);
}

#[test]
fn a_conflicting_write_still_grows_the_mapping() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(0));
    let mut s = shard(&dir, &clock);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .unwrap();
    assert!(
        s.index("a", br#"{"t":"x"}"#, true, &Expect::default())
            .is_err()
    );
    assert!(s.mapping().to_json().contains(r#""t":{"type":"text""#));
}

#[test]
fn a_shard_is_idle_since_its_last_write() {
    let dir = ByteBuffersDirectory::new();
    let clock = Arc::new(AtomicU64::new(1_000));
    let mut s = shard(&dir, &clock);
    assert_eq!(s.idle_millis(), 0, "just opened");
    clock.store(4_000, Ordering::Relaxed);
    assert_eq!(s.idle_millis(), 3_000);
    s.index("a", br#"{"n":1}"#, false, &Expect::default())
        .expect("indexed");
    assert_eq!(s.idle_millis(), 0, "an index is a write");
    clock.store(9_000, Ordering::Relaxed);
    assert_eq!(s.idle_millis(), 5_000);
    s.delete("a", &Expect::default()).expect("deleted");
    assert_eq!(s.idle_millis(), 0, "a delete is a write");
    // A refused write is not a write.
    clock.store(12_000, Ordering::Relaxed);
    let refused = s.index("a", br#"{"n":"x"}"#, false, &Expect::default());
    assert!(refused.is_err());
    assert_eq!(s.idle_millis(), 3_000);
}
