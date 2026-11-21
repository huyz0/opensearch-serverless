//! docs/specs/node-doc-read-routes: a shard's realtime get.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::{Expect, Shard};
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

fn put(s: &mut Shard<'_>, id: &str, source: &str) {
    s.index(id, source.as_bytes(), false, &Expect::default())
        .expect("indexes");
}

#[test]
fn a_realtime_get_sees_a_write_no_refresh_has_made_visible() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    put(&mut s, "a", r#"{"n":1}"#);
    let got = s.get("a", true).expect("reads").expect("found");
    assert_eq!(
        (got.version, got.seq_no, got.primary_term),
        (1, 0, 3),
        "metadata"
    );
    assert_eq!(got.source, br#"{"n":1}"#);
}

#[test]
fn a_get_that_is_not_realtime_reads_the_last_refresh() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    put(&mut s, "a", r#"{"n":1}"#);
    assert!(s.get("a", false).expect("reads").is_none());
    s.refresh().expect("refreshes");
    assert!(s.get("a", false).expect("reads").is_some());
    put(&mut s, "a", r#"{"n":2}"#);
    let old = s.get("a", false).expect("reads").expect("found");
    assert_eq!((old.version, old.source), (1, br#"{"n":1}"#.to_vec()));
    let new = s.get("a", true).expect("reads").expect("found");
    assert_eq!((new.version, new.source), (2, br#"{"n":2}"#.to_vec()));
}

#[test]
fn a_deleted_or_unknown_id_is_not_found() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    assert!(s.get("never", true).expect("reads").is_none());
    put(&mut s, "a", r#"{"n":1}"#);
    s.refresh().expect("refreshes");
    s.delete("a", &Expect::default()).expect("deletes");
    // The segment still holds it until the next refresh: the map knows.
    assert!(s.get("a", true).expect("reads").is_none());
    s.refresh().expect("refreshes");
    assert!(s.get("a", true).expect("reads").is_none());
    assert!(s.get("a", false).expect("reads").is_none());
}

#[test]
fn a_realtime_get_of_an_id_the_map_does_not_hold_does_not_refresh() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    put(&mut s, "a", r#"{"n":1}"#);
    // `a` is unrefreshed, so a get of `b` must leave it that way.
    assert!(s.get("b", true).expect("reads").is_none());
    assert!(s.get("a", false).expect("reads").is_none());
}

#[test]
fn a_realtime_get_of_a_deleted_id_does_not_refresh() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    put(&mut s, "a", r#"{"n":1}"#);
    s.refresh().expect("refreshes");
    put(&mut s, "b", r#"{"n":2}"#);
    s.delete("a", &Expect::default()).expect("deletes");
    assert!(s.get("a", true).expect("reads").is_none());
    // The map answered; `b` is still unrefreshed.
    assert!(s.get("b", false).expect("reads").is_none());
}

#[test]
fn a_routed_document_returns_its_routing_and_one_indexed_without_clears_it() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    s.index_routed("a", br#"{"n":1}"#, Some("r1"), false, &Expect::default())
        .expect("indexes");
    put(&mut s, "b", r#"{"n":2}"#);
    let a = s.get("a", true).expect("reads").expect("found");
    assert_eq!(a.routing.as_deref(), Some("r1"));
    assert_eq!(a.source, br#"{"n":1}"#, "the source is untouched");
    assert_eq!(
        s.get("b", true).expect("reads").expect("found").routing,
        None
    );
    // Still there after a refresh, from the segment.
    s.refresh().expect("refreshes");
    assert_eq!(
        s.get("a", false)
            .expect("reads")
            .expect("found")
            .routing
            .as_deref(),
        Some("r1")
    );
    // A new version without routing has none; with another, the new one.
    put(&mut s, "a", r#"{"n":3}"#);
    assert_eq!(
        s.get("a", true).expect("reads").expect("found").routing,
        None
    );
    s.index_routed("a", br#"{"n":4}"#, Some("r2"), false, &Expect::default())
        .expect("indexes");
    assert_eq!(
        s.get("a", true)
            .expect("reads")
            .expect("found")
            .routing
            .as_deref(),
        Some("r2")
    );
}

#[test]
fn a_replayed_record_keeps_its_routing() {
    let dir = ByteBuffersDirectory::new();
    let mut s = shard(&dir, &Arc::new(AtomicU64::new(0)));
    let record = oss_format::WalRecord::index("a".to_owned(), r#"{"n":1}"#.to_owned(), 0, 3, 1)
        .with_routing(Some("r1"))
        .expect("routing");
    assert!(s.replay(&record).expect("replays"));
    assert_eq!(
        s.get("a", true)
            .expect("reads")
            .expect("found")
            .routing
            .as_deref(),
        Some("r1")
    );
}
