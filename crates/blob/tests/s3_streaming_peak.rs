//! Bodies stream over HTTP too: a 16 MiB object goes to the emulator's S3
//! front end and back through the S3 client and one 64 KiB pooled buffer,
//! and neither side holds a second copy of it (docs/specs/s3-client/SPEC.md
//! AC-13). Its own binary, because the counting allocator is process-wide:
//! the reading covers the client and the server together.

use std::sync::Arc;

use oss_blob::emulator::http::S3Server;
use oss_blob::{
    BlobResult, ByteRange, ChunkSource, Credentials, Emulator, PooledStore, S3Config, S3Op, S3Store,
};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const OBJECT: u64 = 16 << 20;
/// Heap beside the object itself, for both ends of the connection: the
/// client's two 16 KiB connection buffers, the server's read, write and
/// line buffers (40 KiB), request and header strings, and the threads the
/// server starts for a connection. 256 KiB, against the 16 MiB a copy of
/// the body would cost.
const OVERHEAD: usize = 256 << 10;

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
fn a_16_mib_object_streams_over_http_through_one_pooled_buffer() {
    let creds = Credentials::new("peak-access", "peak-secret");
    let emulator = Arc::new(Emulator::new(OBJECT * 2, 1));
    let server = S3Server::start(emulator, "peak-bucket", "us-east-1", creds.clone()).unwrap();
    let client = S3Store::new(S3Config::new(
        &server.endpoint(),
        "peak-bucket",
        "us-east-1",
        creds,
    ))
    .unwrap();
    let config = BudgetConfig::new(1 << 20)
        .with_pool(Pool::Transport, 1 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 1).expect("fits");
    let store = PooledStore::new(client, buffers);
    let object = usize::try_from(OBJECT).expect("fits usize");

    // Writing keeps exactly one copy: the emulator's stored object.
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
        .check_below(object.saturating_add(OVERHEAD))
        .expect("put held a second copy of the body");

    // Reading allocates nothing that grows with the object.
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
        .check_below(OVERHEAD)
        .expect("get allocated in proportion to the object");

    assert_eq!((out.bytes, seen), (OBJECT, OBJECT));
    let expected = (0..OBJECT).fold(0u64, |s, i| s.wrapping_add(i % 251));
    assert_eq!(checksum, expected);
    let meter = oss_blob::BlobStore::meter(store.store());
    assert_eq!(meter.count(S3Op::Put), 1);
    assert_eq!(meter.count(S3Op::Get), 1);
    assert_eq!(server.requests(), 2);
    assert_eq!(store.buffers().available(), 1);
    eprintln!("peak above baseline over HTTP: put {put_peak} bytes, get {get_peak} bytes");
}
