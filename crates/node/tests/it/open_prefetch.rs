//! The open prefetch over the layout the Java shell writes (a segment is one
//! blob, its files ranges of it): what a cold open costs with it on and off.
//! docs/specs/reader-open-prefetch/SPEC.md AC-8.

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, Rounds};
use oss_reader::prefetch::{Gate, OpenPrefetch};
use oss_reader::{ShardFiles, open_located_with};

use crate::support::loaded;

const INDEXES: [&str; 7] = [
    "compat-bool",
    "compat-errors",
    "compat-match",
    "compat-match-all",
    "compat-paging-sort",
    "compat-range",
    "compat-term",
];

/// Requests, bytes read and round trips of one cold open of `index`.
fn cold_open(index: &str, prefetch: &OpenPrefetch) -> (u64, u64, usize) {
    let store = loaded(Rounds::new(Emulator::new(64 << 20, 1)));
    let rounds = store.store();
    rounds.reset(std::time::Duration::from_millis(20));
    let (requests, bytes) = (rounds.meter().total(), rounds.meter().bytes_read());
    let (files, meta) = ShardFiles::open_with_meta(&store, "", index, 0).expect("locates");
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let shard = open_located_with(store.clone(), cache, "", index, 0, files, meta, prefetch)
        .expect("opens");
    assert!(shard.reader.max_doc() > 0, "{index}");
    (
        rounds.meter().total() - requests,
        rounds.meter().bytes_read() - bytes,
        rounds.depth(),
    )
}

#[test]
fn the_shell_s_layout_opens_in_fewer_requests_and_the_same_bytes() {
    let mut report = String::new();
    for index in INDEXES {
        let on = OpenPrefetch {
            gate: Gate::new(32 << 20, 32),
            ..OpenPrefetch::default()
        };
        let (on_requests, on_bytes, on_depth) = cold_open(index, &on);
        let (off_requests, off_bytes, off_depth) = cold_open(index, &OpenPrefetch::off());
        report.push_str(&format!(
            "{index}: requests {off_requests} to {on_requests}, depth {off_depth} to {on_depth}, bytes {off_bytes} to {on_bytes}\n"
        ));
        assert!(on_requests < off_requests, "{report}");
        assert!(on_depth <= off_depth, "{report}");
        // No gap is spanned and no file the open leaves alone is read.
        assert_eq!(on_bytes, off_bytes, "{report}");
    }
    eprintln!("{report}");
}
