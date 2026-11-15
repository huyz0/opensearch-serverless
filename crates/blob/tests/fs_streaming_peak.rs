//! Bodies stream on the filesystem store too: a 16 MiB object goes in and
//! out through one 64 KiB pooled buffer and no call holds a copy of it
//! (docs/specs/fs-store/SPEC.md AC-11). Its own binary, because the
//! counting allocator is process-wide.

use oss_blob::{BlobResult, BlobStore, ByteRange, ChunkSource, FsStore, PooledStore, S3Op};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::{CountingAllocator, PeakWindow, TempDir, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const OBJECT: u64 = 16 << 20;
/// Heap beyond the start point: the pooled buffer, paths, a staged name.
const BOUND: usize = 256 << 10;

/// A body produced on the fly, so the test itself holds none of it.
struct Pattern {
    len: u64,
    sent: u64,
}

impl ChunkSource for Pattern {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let left = usize::try_from(self.len.saturating_sub(self.sent)).unwrap_or(usize::MAX);
        let n = left.min(buf.len());
        for (i, b) in buf.iter_mut().take(n).enumerate() {
            *b = (self.sent.wrapping_add(i as u64) % 251) as u8;
        }
        self.sent = self.sent.saturating_add(n as u64);
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.sent = 0;
        Ok(())
    }
}

#[test]
fn a_16_mib_object_streams_through_one_pooled_buffer() {
    let dir = TempDir::new("fs-peak");
    let config = BudgetConfig::new(1 << 20)
        .with_pool(Pool::Transport, 1 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 1).expect("fits");
    let store = PooledStore::new(FsStore::new(dir.path()).expect("open"), buffers);

    let window = PeakWindow::start(&ALLOC);
    store
        .put(
            "seg/0",
            &mut Pattern {
                len: OBJECT,
                sent: 0,
            },
        )
        .expect("put");
    let put_peak = window
        .check_below(BOUND)
        .expect("put allocated in proportion to the object");

    let mut checksum = 0u64;
    let mut seen = 0u64;
    let window = PeakWindow::start(&ALLOC);
    let out = store
        .get("seg/0", ByteRange::Full, &mut |chunk: &[u8]| {
            assert!(chunk.len() <= SMALL_BUFFER);
            seen = seen.saturating_add(chunk.len() as u64);
            checksum = chunk
                .iter()
                .fold(checksum, |s, b| s.wrapping_add(u64::from(*b)));
            Ok(())
        })
        .expect("get");
    let get_peak = window
        .check_below(BOUND)
        .expect("get allocated in proportion to the object");

    assert_eq!((out.bytes, seen), (OBJECT, OBJECT));
    let expected = (0..OBJECT).fold(0u64, |s, i| s.wrapping_add(i % 251));
    assert_eq!(checksum, expected);
    assert_eq!(store.store().meter().count(S3Op::Put), 1);
    assert_eq!(store.store().meter().count(S3Op::Get), 1);
    eprintln!("peak above start: put {put_peak} bytes, get {get_peak} bytes");
}
