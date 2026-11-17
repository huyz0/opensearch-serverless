//! A 64 MiB segment publishes by streaming: the writer holds no copy of it. Its own binary, because the counting
//! allocator is process-wide.

use lucene_store::MmapDirectory;
use oss_blob::{BlobStore, FsStore};
use oss_format::ShardRef;
use oss_testkit::{CountingAllocator, PeakWindow, TempDir, counting_allocator};
use oss_writer::publish::ShardPublisher;
use std::io::Write;
use std::sync::Arc;

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const SEGMENT: usize = 64 << 20;
/// Heap the publish may hold: the header, the manifest, one 64 KiB
/// buffer and the bookkeeping.
const BOUND: usize = 2 << 20;

#[test]
fn a_64_mib_segment_publishes_with_peak_heap_below_2_mib() {
    // The segment sits on disk and is mapped, as a shard's is.
    let source = TempDir::new("publish-memory-shard");
    let chunk = [7u8; 1 << 16];
    let mut file = std::fs::File::create(source.path().join("_0.cfs")).expect("file");
    for _ in 0..(SEGMENT / chunk.len()) {
        file.write_all(&chunk).expect("written");
    }
    std::fs::write(source.path().join("segments_1"), b"commit").expect("segments");
    let dir = MmapDirectory::open(source.path());
    let temp = TempDir::new("publish-memory");
    let store = Arc::new(FsStore::new(temp.path()).expect("store")) as Arc<dyn BlobStore>;
    let shard = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut publisher = ShardPublisher::new(store, "", &shard).expect("publisher");
    let files = vec!["_0.cfs".to_owned(), "segments_1".to_owned()];

    let window = PeakWindow::start(&ALLOC);
    let published = publisher
        .publish(&dir, &files, 3, "node-1", 0, 1)
        .expect("published");
    window
        .check_below(BOUND)
        .expect("the publish held a copy of the segment");
    assert_eq!(published.blobs.len(), 1);
}
