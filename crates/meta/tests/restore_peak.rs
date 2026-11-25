//! docs/specs/snapshot-restore AC-4: restoring a file of 48 MiB through two
//! buffers of 1 MiB holds less than 64 KiB beyond those buffers, measured by a
//! counting allocator. Its own binary, so no other test shares the counters.

use std::sync::Arc;

use oss_blob::{BlobResult, BlobStore, ChunkSource, FsStore};
use oss_format::{RestoreIndex, ShardRef};
use oss_meta::{CopyBuffers, DeleteMode, DescriptorStore, SnapshotRestorer};
use oss_testkit::{CountingAllocator, PeakWindow, TempDir, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const FILE: u64 = 48 << 20;
const CHUNK: usize = 1 << 20;
/// What the restore may hold beyond its two buffers: a manifest and a few names.
const LIMIT: usize = 64 << 10;

const RECORD: &str = r#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
  "indices":{"logs":{"uuid":"U1","number_of_shards":1,
    "shards":[{"shard":0,"term":1,"files":{"_0.si":"t=1"},"lengths":{"_0.si":50331648}}]}}}"#;

/// A body of `len` bytes made as it is read, so the test holds none of it.
struct Pattern {
    len: u64,
    done: u64,
}

impl ChunkSource for Pattern {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let n = usize::try_from(self.len.saturating_sub(self.done))
            .unwrap_or(usize::MAX)
            .min(buf.len());
        buf[..n].fill(7);
        self.done = self.done.saturating_add(n as u64);
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.done = 0;
        Ok(())
    }
}

#[test]
fn a_file_of_48_mib_is_restored_holding_under_64_kib_beyond_the_two_buffers() {
    let dir = TempDir::new("restore-peak");
    let store: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()).expect("store"));
    let from = ShardRef {
        index: "logs",
        uuid: "U1",
        shard: 0,
    };
    let key = format!("{}t=1/_0.si", from.base_path("").unwrap());
    store
        .put(
            &key,
            &mut Pattern { len: FILE, done: 0 },
            &mut vec![0u8; CHUNK],
        )
        .expect("seeded");
    let descriptors =
        DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional).unwrap();
    let source = RestoreIndex::find(RECORD.as_bytes(), "logs")
        .unwrap()
        .unwrap();
    let (mut read, mut write) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut record = vec![0u8; 1 << 16];
    let restorer = SnapshotRestorer::new(Arc::clone(&store), "");

    let window = PeakWindow::start(&ALLOC);
    restorer
        .restore_index(
            &descriptors,
            &source,
            "copy",
            "U2",
            1,
            &mut record,
            &mut CopyBuffers {
                read: &mut read,
                write: &mut write,
            },
        )
        .expect("restored");
    let peak = window.peak_bytes();
    assert!(
        peak < LIMIT,
        "peak {peak} bytes beyond the two buffers, for a file of {FILE}"
    );
    let to = ShardRef {
        index: "copy",
        uuid: "U2",
        shard: 0,
    };
    let mut seen = 0u64;
    store
        .get(
            &format!("{}t=1/_0.si", to.base_path("").unwrap()),
            oss_blob::ByteRange::Full,
            &mut vec![0u8; CHUNK],
            &mut |c: &[u8]| {
                seen += c.len() as u64;
                Ok(())
            },
        )
        .expect("copied");
    assert_eq!(seen, FILE);
}
