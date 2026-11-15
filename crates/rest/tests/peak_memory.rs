//! Peak memory under concurrent large bodies: sixteen clients each upload a
//! 4 MiB bulk body at once (64 MiB in all) to a server with eight workers and
//! eight 64 KiB body buffers, and the handler tokenizes every line as it
//! streams. The process's heap peak stays within what the server charged to
//! the request-body pool, plus what the handlers reserved, plus a fixed
//! allowance, which is a small fraction of one body. Its own binary, because
//! the counting allocator is process-wide.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use oss_mem::{BudgetConfig, MemoryBudget, Pool, SMALL_BUFFER};
use oss_rest::json::{Limits, Token, TokenSink, Tokenizer};
use oss_rest::ndjson::{LineSink, Splitter};
use oss_rest::{JSON, Method, Request, Response, RestError, Router, Server, ServerConfig};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const CLIENTS: usize = 16;
const BODY: usize = 4 << 20;
/// Heap outside the pool: thread handles and names, the client threads, the
/// listener, the router. Measured at about 155 KiB in a debug build.
const ALLOWANCE: usize = 512 << 10;

const LIMITS: Limits = Limits {
    max_depth: 32,
    max_key_bytes: 256,
    max_string_bytes: 1 << 20,
    max_number_bytes: 64,
    string_chunk: 1024,
};

struct Count(usize);

impl TokenSink for Count {
    type Error = RestError;
    fn token(&mut self, _: Token<'_>) -> Result<(), RestError> {
        self.0 = self.0.saturating_add(1);
        Ok(())
    }
}

struct Lines {
    t: Tokenizer,
    tokens: Count,
}

impl LineSink for Lines {
    type Error = RestError;
    fn part(&mut self, b: &[u8]) -> Result<(), RestError> {
        self.t.feed(b, &mut self.tokens)
    }
    fn end_line(&mut self, _: u64) -> Result<(), RestError> {
        self.t.finish(&mut self.tokens)?;
        self.t.reset();
        Ok(())
    }
}

fn bulk(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    // The tokenizer's scratch and stack, reserved before they are allocated.
    req.body()
        .reserve(LIMITS.max_key_bytes + LIMITS.max_depth)?;
    let mut lines = Lines {
        t: Tokenizer::new(LIMITS),
        tokens: Count(0),
    };
    let mut split = Splitter::new();
    while let Some(chunk) = req.body().next_chunk()? {
        split.feed(chunk, &mut lines)?;
    }
    split.finish()?;
    let mut out = [0u8; 64];
    let n = {
        let mut w = &mut out[..];
        write!(w, "{{\"tokens\":{}}}", lines.tokens.0).map_err(RestError::from)?;
        64usize.saturating_sub(w.len())
    };
    resp.send(200, JSON, &out[..n])
}

/// One 64 KiB block of bulk lines, sent over and over.
fn block() -> Arc<[u8]> {
    let mut b = Vec::with_capacity(SMALL_BUFFER);
    let line = b"{\"index\":{\"_id\":\"0123456789\"}}\n{\"title\":\"hello world\",\"n\":42}\n";
    while b.len().saturating_add(line.len()) <= SMALL_BUFFER {
        b.extend_from_slice(line);
    }
    b.into()
}

fn upload(addr: std::net::SocketAddr, block: &[u8], ok: &AtomicUsize) {
    let blocks = BODY.checked_div(block.len()).expect("a block");
    let mut s = TcpStream::connect(addr).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(60)))
        .expect("timeout");
    let mut head = [0u8; 160];
    let n = {
        let mut w = &mut head[..];
        write!(
            w,
            "POST /_bulk HTTP/1.1\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            blocks.saturating_mul(block.len())
        )
        .expect("fits");
        160usize.saturating_sub(w.len())
    };
    s.write_all(&head[..n]).expect("head");
    for _ in 0..blocks {
        s.write_all(block).expect("body");
    }
    let mut reply = [0u8; 512];
    let mut got = 0;
    loop {
        match s.read(&mut reply[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got = got.saturating_add(n),
        }
    }
    if reply[..got].starts_with(b"HTTP/1.1 200 ") {
        ok.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
fn sixteen_concurrent_large_bodies_stay_within_the_charged_buffers() {
    let block = block();
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server_config = ServerConfig {
        workers: 8,
        max_connections: 64,
        body_buffers: 8,
        ..ServerConfig::default()
    };
    let window = PeakWindow::start(&ALLOC);
    let router = Router::new()
        .route(Method::Post, "/_bulk", &[], bulk)
        .expect("routes");
    let server = Server::start(listener, server_config, router, &budget).expect("starts");
    let charged = budget.in_use(Pool::RequestBodies);
    let addr = server.local_addr();
    let ok = Arc::new(AtomicUsize::new(0));
    let clients: Vec<_> = (0..CLIENTS)
        .map(|_| {
            let (block, ok) = (Arc::clone(&block), Arc::clone(&ok));
            thread::spawn(move || upload(addr, &block, &ok))
        })
        .collect();
    for c in clients {
        c.join().expect("client");
    }
    let peak = window.peak_bytes();
    server.shutdown();
    assert_eq!(
        ok.load(Ordering::Acquire),
        CLIENTS,
        "every upload served, none refused"
    );
    let reserved = 8 * (LIMITS.max_key_bytes + LIMITS.max_depth);
    let bound = charged + reserved + ALLOWANCE;
    println!(
        "peak {peak} bytes; charged {charged}, bound {bound}, bodies {}",
        CLIENTS * BODY
    );
    assert!(peak <= bound, "peak {peak} over {bound}");
    assert!(bound < BODY / 2, "the bound is a fraction of one body");
}
