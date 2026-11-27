//! A merge that fails is skipped and counted: the refresh that ran it
//! succeeds, no document is lost, and later refreshes merge again.

use std::collections::BTreeSet;
use std::io::Error as IoError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lucene_store::{ByteBuffersDirectory, Directory, Error, FsIndexOutput, Input, Lock, Result};
use oss_writer::index::knn::open_writer;
use oss_writer::index::{Expect, MergeGate, MergeGuard, MergeSettings, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

/// A directory that refuses to create files while `broken` is set.
struct Flaky {
    inner: ByteBuffersDirectory,
    broken: Arc<AtomicBool>,
}

impl Directory for Flaky {
    fn list_all(&self) -> Result<Vec<String>> {
        self.inner.list_all()
    }
    fn open(&self, name: &str) -> Result<Input> {
        self.inner.open(name)
    }
    fn file_length(&self, name: &str) -> Result<u64> {
        self.inner.file_length(name)
    }
    fn create_output(&self, name: &str) -> Result<FsIndexOutput> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(Error::Io(IoError::other("no space left")));
        }
        self.inner.create_output(name)
    }
    fn sync(&self, names: &[String]) -> Result<()> {
        self.inner.sync(names)
    }
    fn rename(&self, source: &str, dest: &str) -> Result<()> {
        self.inner.rename(source, dest)
    }
    fn delete_file(&self, name: &str) -> Result<()> {
        self.inner.delete_file(name)
    }
    fn sync_meta_data(&self) -> Result<()> {
        self.inner.sync_meta_data()
    }
    fn obtain_lock(&self, name: &str) -> Result<Box<dyn Lock>> {
        self.inner.obtain_lock(name)
    }
    fn pending_deletions(&self) -> Result<BTreeSet<String>> {
        self.inner.pending_deletions()
    }
}

/// Breaks the directory when a merge starts, and mends it when the merge ends.
struct Breaker {
    broken: Arc<AtomicBool>,
    arm: AtomicBool,
    asked: AtomicUsize,
}

struct Mend(Arc<AtomicBool>);

impl Drop for Mend {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl MergeGate for Breaker {
    fn try_start(&self, _: u64) -> Option<Box<dyn MergeGuard>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.arm.swap(false, Ordering::SeqCst) {
            self.broken.store(true, Ordering::SeqCst);
        }
        Some(Box::new(Mend(Arc::clone(&self.broken))))
    }
}

fn write_refresh(shard: &mut Shard<'_>, i: u32) {
    let body = format!("{{\"n\":{i}}}");
    shard
        .index(&i.to_string(), body.as_bytes(), false, &Expect::default())
        .expect("indexed");
    shard
        .refresh()
        .expect("a failed merge does not fail the refresh");
}

// docs/specs/writer-segment-merge AC-2
#[test]
fn a_merge_that_fails_is_skipped_and_counted_and_the_next_one_runs() {
    let broken = Arc::new(AtomicBool::new(false));
    let dir = Flaky {
        inner: ByteBuffersDirectory::new(),
        broken: Arc::clone(&broken),
    };
    let mapping = Mapping::parse_pinned(
        br#"{"properties":{"n":{"type":"long"}}}"#,
        ParseOptions::default(),
    )
    .expect("mapping");
    let (writer, derived) = open_writer(&dir, &mapping).expect("writer");
    let gate = Arc::new(Breaker {
        broken: Arc::clone(&broken),
        arm: AtomicBool::new(false),
        asked: AtomicUsize::new(0),
    });
    let mut shard = Shard::new(&dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived)
        .with_merging(
            MergeSettings {
                segments_per_tier: 4,
                max_merge_at_once: 4,
                max_merged_segment_bytes: 64 << 20,
            },
            Arc::clone(&gate) as Arc<dyn MergeGate>,
        );
    for i in 0..4 {
        write_refresh(&mut shard, i);
    }
    // The next merge fails while it writes its segment.
    gate.arm.store(true, Ordering::SeqCst);
    for i in 4..8 {
        write_refresh(&mut shard, i);
    }
    assert_eq!(shard.merge_failures(), 1, "the failed merge was counted");
    for i in 8..40 {
        write_refresh(&mut shard, i);
    }
    assert_eq!(shard.merge_failures(), 1, "later merges did not fail");
    let segments = lucene_search::directory_reader::DirectoryReader::open(&dir)
        .expect("open")
        .segment_readers()
        .len();
    assert!(segments < 16, "later merges ran: {segments} segments");
    for i in 0..40 {
        assert!(
            shard.get(&i.to_string(), false).expect("get").is_some(),
            "{i} lost"
        );
    }
}
