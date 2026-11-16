//! A full result window over an index the engine built: a search of size
//! 10,000 sorted by `n` over 12,000 documents in three segments answers
//! every hit, in order, and the process's heap peak while it runs stays
//! within what the search reserved for its hits plus a fixed allowance;
//! with a request-body pool short of the window, the search is refused.
//! Its own binary, because the counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_reader::dsl::{MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_reader::open_shard;
use oss_reader::search::hits_bytes;
use oss_testkit::index::{Spec, blobs};
use oss_testkit::{CountingAllocator, PeakWindow, TempDir, counting_allocator};

use support::{request, start_with, store};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const BODY: &[u8] = br#"{"size":10000,"sort":[{"n":"asc"}]}"#;
const DOCS: usize = 12_000;
/// Heap besides the hits: the request's threads and buffers, the engine's
/// state for the search, and the client's sort values (80 KB). The client
/// reads the reply as it streams and keeps no copy of it.
const ALLOWANCE: usize = 1 << 20;

/// What the server reserves for the window's hits.
fn reserved_for_hits(store: &Arc<PooledStore<Emulator>>) -> usize {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokenizer = source::tokenizer();
    tokenizer.feed(BODY, &mut rec).expect("feeds");
    tokenizer.finish(&mut rec).expect("ends");
    let source = SearchSource::parse(&rec).expect("parses");
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    let shard = open_shard(Arc::clone(store), cache, "", "window", 0).expect("opens");
    hits_bytes(&source, &shard)
}

/// Finds each hit's `"sort":[n]`, and the capped total, in a reply's
/// bytes as they stream.
#[derive(Default)]
struct Sorts {
    window: Vec<u8>,
    number: Option<i64>,
    values: Vec<i64>,
    capped_total: bool,
}

impl Sorts {
    const KEY: &'static [u8] = br#""sort":["#;
    const TOTAL: &'static [u8] = br#""total":{"value":10000,"relation":"gte"}"#;

    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if let Some(n) = self.number {
                if b.is_ascii_digit() {
                    self.number = Some(n * 10 + i64::from(b - b'0'));
                    continue;
                }
                self.values.push(n);
                self.number = None;
            }
            self.window.push(b);
            if self.window.len() > Self::TOTAL.len() {
                self.window.remove(0);
            }
            if self.window.ends_with(Self::KEY) {
                self.number = Some(0);
            }
            self.capped_total |= self.window.ends_with(Self::TOTAL);
        }
    }
}

/// Sends `body` and reads the chunked reply as it streams, keeping only
/// the status and each hit's sort value.
fn stream(addr: SocketAddr, body: &[u8]) -> (u16, Sorts) {
    let mut s = TcpStream::connect(addr).expect("connects");
    let head = format!(
        "POST /window/_search HTTP/1.1\r\nhost: test\r\nconnection: close\r\n\
         content-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).expect("sends head");
    s.write_all(body).expect("sends body");
    let mut reader = std::io::BufReader::with_capacity(16 << 10, s);
    let mut line = String::new();
    let read_line = |reader: &mut std::io::BufReader<TcpStream>, line: &mut String| {
        line.clear();
        std::io::BufRead::read_line(reader, line).expect("reads a line");
    };
    read_line(&mut reader, &mut line);
    let status: u16 = line[9..12].parse().expect("status");
    let mut chunked = false;
    loop {
        read_line(&mut reader, &mut line);
        if line == "\r\n" {
            break;
        }
        chunked |= line.eq_ignore_ascii_case("transfer-encoding: chunked\r\n");
    }
    let mut sorts = Sorts::default();
    let mut buf = [0u8; 16 << 10];
    if !chunked {
        loop {
            match reader.read(&mut buf).expect("reads") {
                0 => break,
                n => sorts.feed(&buf[..n]),
            }
        }
        return (status, sorts);
    }
    loop {
        read_line(&mut reader, &mut line);
        let mut left = usize::from_str_radix(line.trim_end(), 16).expect("chunk size");
        if left == 0 {
            break;
        }
        while left > 0 {
            let take = left.min(buf.len());
            let n = reader.read(&mut buf[..take]).expect("reads");
            assert!(n > 0, "the reply ended inside a chunk");
            sorts.feed(&buf[..n]);
            left -= n;
        }
        read_line(&mut reader, &mut line);
    }
    (status, sorts)
}

fn budget(request_bodies: usize) -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(1 << 30)
            .with_pool(Pool::RequestBodies, request_bodies)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 64 << 20))
            .expect("fits"),
    )
}

/// Set in the child process that builds the index, which the test runs
/// so the engine's writing is not in the parent's heap peak.
const CHILD: &str = "OSS_FULL_WINDOW_TEST_CHILD";

/// Writes the index's blobs under `dir`: `keys` lists their keys in order,
/// and blob `i` is the file `i`.
fn write_blobs(dir: &Path) {
    let blobs = blobs("", &Spec::new("window", DOCS).segments_of(DOCS / 3));
    let keys: Vec<&str> = blobs.iter().map(|(k, _)| k.as_str()).collect();
    std::fs::write(dir.join("keys"), keys.join("\n")).expect("keys");
    for (i, (_, bytes)) in blobs.iter().enumerate() {
        std::fs::write(dir.join(i.to_string()), bytes).expect("blob");
    }
}

/// Builds the index in a child process and loads its blobs into `store`.
fn load_built(store: &PooledStore<Emulator>) {
    let dir = TempDir::new("full-window");
    let me = std::env::current_exe().expect("this test binary");
    let status = std::process::Command::new(me)
        .args(["--exact", "a_full_window_stays_within_its_reservation"])
        .env(CHILD, dir.path())
        .status()
        .expect("runs the child");
    assert!(status.success(), "the child failed");
    let keys = std::fs::read_to_string(dir.path().join("keys")).expect("keys");
    for (i, key) in keys.lines().enumerate() {
        let bytes = std::fs::read(dir.path().join(i.to_string())).expect("blob");
        store.put(key, &mut SliceSource::new(&bytes)).expect("put");
    }
}

#[test]
fn a_full_window_stays_within_its_reservation() {
    if let Some(dir) = std::env::var_os(CHILD) {
        write_blobs(Path::new(&dir));
        return;
    }
    let store = store();
    load_built(&store);
    let hits = reserved_for_hits(&store);
    assert!(hits > 0);

    // Enough for the server's buffers, a recording and the window. A
    // search of one hit opens the index and warms what every search needs;
    // a second is measured, then the window. The client keeps no copy of
    // a reply. The counting allocator's maximum cannot be reset, so each
    // reading is the highest the process reached, less what was live at
    // its start: the window's reading is its own because it is the larger.
    let node = start_with(Arc::clone(&store), &budget(64 << 20));
    let one = br#"{"size":1,"sort":[{"n":"asc"}]}"#;
    assert_eq!(stream(node.addr(), one).0, 200);
    let before = PeakWindow::start(&ALLOC);
    assert_eq!(stream(node.addr(), one).0, 200);
    let base = before.peak_bytes();
    let window = PeakWindow::start(&ALLOC);
    let (status, sorts) = stream(node.addr(), BODY);
    let peak = window.peak_bytes();
    assert_eq!(status, 200);
    let want: Vec<i64> = (0..10_000).collect();
    assert_eq!(sorts.values, want, "every hit of the window, in n order");
    assert!(sorts.capped_total, "the total is capped at 10,000");
    println!("peak {peak} B, one hit {base} B; hits reserved {hits} B");
    assert!(
        peak.saturating_sub(base) <= hits,
        "the window took {} B beyond a one-hit search, over the {hits} B reserved",
        peak - base
    );
    assert!(
        peak <= hits + ALLOWANCE,
        "peak {peak} over the {hits} B reserved for hits plus {ALLOWANCE}"
    );
    node.server.shutdown();

    // The server's own buffers, and the window short of one byte.
    let probe_budget = budget(64 << 20);
    let probe = start_with(Arc::clone(&store), &probe_budget);
    let buffers = probe_budget.in_use(Pool::RequestBodies);
    probe.server.shutdown();
    let short = start_with(store, &budget(buffers + hits - 1));
    let r = request(short.addr(), "POST", "/window/_search", Some(BODY));
    assert_eq!(r.status, 429, "{:.300}", r.text());
    assert!(
        r.text().contains("circuit_breaking_exception"),
        "{}",
        r.text()
    );
    short.server.shutdown();
}
