//! A 64 MiB pack streams: the writer holds no copy of its parts. Its own
//! binary, because the counting allocator is process-wide.

// ARITH: the offsets compared are two known parts of 32 MiB.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use oss_blob::{BlobResult, BlobStore, ChunkSource, FsStore};
use oss_testkit::{CountingAllocator, PeakWindow, TempDir, counting_allocator};
use oss_writer::pack::{PackStore, Part};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const PART: u64 = 32 << 20;
/// Heap the write may hold: the header, one 64 KiB buffer and the bookkeeping.
const BOUND: usize = 2 << 20;

/// `len` bytes of one value, produced as they are asked for.
struct Run {
    len: u64,
    left: u64,
}

impl ChunkSource for Run {
    fn len(&self) -> u64 {
        self.len
    }
    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let n = usize::try_from(self.left).map_or(buf.len(), |l| l.min(buf.len()));
        buf[..n].fill(7);
        self.left = self.left.saturating_sub(n as u64);
        Ok(n)
    }
    fn rewind(&mut self) -> BlobResult<()> {
        self.left = self.len;
        Ok(())
    }
}

fn part(shard: u32) -> Part {
    Part {
        index_uuid: "u1".into(),
        index_name: "logs".into(),
        shard,
        name: "_0.seg".into(),
        source: Box::new(Run {
            len: PART,
            left: PART,
        }),
    }
}

#[test]
fn a_64_mib_pack_writes_with_peak_heap_below_2_mib() {
    let temp = TempDir::new("pack-memory");
    let store = Arc::new(FsStore::new(temp.path()).expect("store")) as Arc<dyn BlobStore>;
    let packs = PackStore::new(store, "", "node-1");
    let parts = vec![part(0), part(1)];
    let window = PeakWindow::start(&ALLOC);
    let landed = packs.write(1, 0, parts).expect("written");
    window
        .check_below(BOUND)
        .expect("the pack write held a copy of its parts");
    assert_eq!(landed.len(), 2);
    assert_eq!(landed[1].offset - landed[0].offset, PART);
}
