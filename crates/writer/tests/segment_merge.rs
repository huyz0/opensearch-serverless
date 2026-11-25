//! A shard merges its small segments after a refresh under a guard the node
//! grants, and a refused guard skips the merge without failing the refresh.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::knn::open_writer;
use oss_writer::index::{Expect, MergeGate, MergeGuard, MergeSettings, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

const CAP: u64 = 64 << 20;

#[derive(Default)]
struct Gate {
    refuse: bool,
    asked: AtomicUsize,
    live: Arc<AtomicUsize>,
    largest: AtomicU64,
}

struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl MergeGate for Gate {
    fn try_start(&self, bytes: u64) -> Option<Box<dyn MergeGuard>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.largest.fetch_max(bytes, Ordering::SeqCst);
        if self.refuse {
            return None;
        }
        self.live.fetch_add(1, Ordering::SeqCst);
        Some(Box::new(Held(Arc::clone(&self.live))))
    }
}

fn settings() -> MergeSettings {
    MergeSettings {
        segments_per_tier: 8,
        max_merge_at_once: 10,
        max_merged_segment_bytes: CAP,
    }
}

fn shard<'d>(dir: &'d ByteBuffersDirectory, gate: Option<Arc<Gate>>) -> Shard<'d> {
    shard_with(dir, gate, settings())
}

fn shard_with<'d>(
    dir: &'d ByteBuffersDirectory,
    gate: Option<Arc<Gate>>,
    merge: MergeSettings,
) -> Shard<'d> {
    let mapping = Mapping::parse_pinned(
        br#"{"properties":{"n":{"type":"long"}}}"#,
        ParseOptions::default(),
    )
    .expect("mapping");
    let (writer, derived) = open_writer(dir, &mapping).expect("writer");
    let shard = Shard::new(dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived);
    match gate {
        Some(g) => shard.with_merging(merge, g),
        None => shard,
    }
}

fn segments(dir: &ByteBuffersDirectory) -> usize {
    DirectoryReader::open(dir)
        .expect("open")
        .segment_readers()
        .len()
}

fn write_refreshing(shard: &mut Shard<'_>, count: u32) {
    for i in 0..count {
        let body = format!("{{\"n\":{i}}}");
        shard
            .index(&i.to_string(), body.as_bytes(), false, &Expect::default())
            .expect("indexed");
        shard.refresh().expect("refresh");
    }
}

// docs/specs/writer-segment-merge AC-5
#[test]
fn without_merging_every_refresh_leaves_a_segment() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, None);
    write_refreshing(&mut shard, 40);
    assert_eq!(segments(&dir), 40);
}

// docs/specs/writer-segment-merge AC-1, AC-2
#[test]
fn merging_bounds_the_segments_and_keeps_every_document() {
    let dir = ByteBuffersDirectory::new();
    let gate = Arc::new(Gate::default());
    let mut shard = shard(&dir, Some(Arc::clone(&gate)));
    write_refreshing(&mut shard, 100);
    shard
        .index("5", br#"{"n":500}"#, false, &Expect::default())
        .expect("updated");
    shard.delete("7", &Expect::default()).expect("deleted");
    for i in 100..200 {
        let body = format!("{{\"n\":{i}}}");
        shard
            .index(&i.to_string(), body.as_bytes(), false, &Expect::default())
            .expect("indexed");
        shard.refresh().expect("refresh");
    }

    assert!(segments(&dir) <= 24, "{} segments", segments(&dir));
    let mut found = 0;
    for i in 0..200 {
        if shard.get(&i.to_string(), false).expect("get").is_some() {
            found += 1;
        }
    }
    assert_eq!(found, 199, "every id but the deleted one");
    assert!(shard.get("7", false).expect("get").is_none());
    let five = shard.get("5", false).expect("get").expect("found");
    assert_eq!(five.source, br#"{"n":500}"#);
    assert_eq!(gate.live.load(Ordering::SeqCst), 0, "guard released");
}

// docs/specs/writer-segment-merge AC-2
#[test]
fn a_merge_is_durable_and_keeps_the_sequence_numbers() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, Some(Arc::new(Gate::default())));
    write_refreshing(&mut shard, 60);
    let max = shard.max_seq_no();
    assert!(segments(&dir) < 60, "the merge reached the commit");
    drop(shard);
    let again = self::shard(&dir, None);
    assert_eq!(again.max_seq_no(), max);
    let mut again = again;
    assert!(again.get("59", false).expect("get").is_some());
    assert!(segments(&dir) < 60);
}

// docs/specs/writer-segment-merge AC-4
#[test]
fn a_refused_gate_skips_the_merge_and_the_refresh_succeeds() {
    let dir = ByteBuffersDirectory::new();
    let gate = Arc::new(Gate {
        refuse: true,
        ..Gate::default()
    });
    let mut shard = shard(&dir, Some(Arc::clone(&gate)));
    write_refreshing(&mut shard, 30);
    assert_eq!(segments(&dir), 30);
    assert!(gate.asked.load(Ordering::SeqCst) > 0, "the gate was asked");
}

// docs/specs/writer-segment-merge AC-4
#[test]
fn the_reservation_asked_for_is_twice_the_smaller_of_the_cap_and_the_shard() {
    let dir = ByteBuffersDirectory::new();
    let gate = Arc::new(Gate::default());
    let mut shard = shard(&dir, Some(Arc::clone(&gate)));
    write_refreshing(&mut shard, 30);
    let asked = gate.largest.load(Ordering::SeqCst);
    assert!(asked > 0 && asked <= 2 * CAP, "{asked}");
}

/// What a searcher counts: documents not soft deleted, as the reader crate
/// opens a shard.
fn live_docs(dir: &ByteBuffersDirectory) -> usize {
    let reader = DirectoryReader::open(dir)
        .and_then(|r| r.soft_deletes_wrapper("__soft_deletes"))
        .expect("open");
    usize::try_from(reader.num_docs()).expect("count")
}

// docs/specs/writer-segment-merge AC-2
#[test]
fn a_searcher_counts_every_live_document_after_merges_with_updates() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, Some(Arc::new(Gate::default())));
    let mut expected = std::collections::BTreeSet::new();
    for i in 0..300u32 {
        let id = if i % 3 == 0 { i / 2 } else { i };
        expected.insert(id);
        let body = format!("{{\"n\":{i}}}");
        shard
            .index(&id.to_string(), body.as_bytes(), false, &Expect::default())
            .expect("indexed");
        shard.refresh().expect("refresh");
    }
    assert!(segments(&dir) < 300, "merged");
    assert_eq!(live_docs(&dir), expected.len());
}

// docs/specs/writer-segment-merge AC-4
#[test]
fn segments_the_cap_leaves_alone_never_ask_the_gate() {
    let dir = ByteBuffersDirectory::new();
    let gate = Arc::new(Gate::default());
    // A cap below every segment's size: none can merge.
    let tiny = MergeSettings {
        max_merged_segment_bytes: 64,
        ..settings()
    };
    let mut shard = shard_with(&dir, Some(Arc::clone(&gate)), tiny);
    write_refreshing(&mut shard, 30);
    assert_eq!(gate.asked.load(Ordering::SeqCst), 0, "nothing to merge");
    assert_eq!(segments(&dir), 30);
    assert_eq!(shard.merge_failures(), 0);
}
