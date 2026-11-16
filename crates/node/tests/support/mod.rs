//! A node serving the compat indexes the Java shell wrote
//! (fixtures/compat-blobs) from the in-process emulator, and a minimal
//! HTTP client.

// ARITH: reply offsets and sizes are small; an overflow panics the test,
// which fails it as it should.
#![allow(dead_code, clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, S3Op, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_node::search::{SearchService, routes, start_reader};
use oss_rest::{Router, Server, ServerConfig};

/// The node id shard failures name.
pub const NODE: &str = "node-1";

fn blobs_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat-blobs")
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("fixture dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.file_name().is_some_and(|n| n != "README.md") {
            let key = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((key, std::fs::read(&path).expect("read fixture")));
        }
    }
}

/// An emulator holding every compat blob at its key.
pub fn store() -> Arc<PooledStore<Emulator>> {
    loaded(Emulator::new(64 << 20, 1))
}

/// `inner` holding every compat blob at its key, behind a buffer pool.
pub fn loaded<S: BlobStore>(inner: S) -> Arc<PooledStore<S>> {
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 16).expect("fits");
    let store = PooledStore::new(inner, buffers);
    let mut blobs = Vec::new();
    let root = blobs_root();
    walk(&root, &root, &mut blobs);
    assert!(blobs.len() >= 14, "compat blobs missing: {}", blobs.len());
    for (key, bytes) in &blobs {
        store
            .put(key, &mut SliceSource::new(bytes))
            .expect("load blob");
    }
    Arc::new(store)
}

/// Every request kind's count so far.
pub fn counts(store: &PooledStore<Emulator>) -> [u64; 7] {
    S3Op::ALL.map(|op| store.store().meter().count(op))
}

/// A running node.
pub struct Node {
    pub server: Server,
    pub store: Arc<PooledStore<Emulator>>,
    pub service: Arc<SearchService<Emulator>>,
}

impl Node {
    pub fn addr(&self) -> SocketAddr {
        self.server.local_addr()
    }
}

/// The server's sizing for tests: a few workers.
pub fn server_config() -> ServerConfig {
    ServerConfig {
        workers: 4,
        max_connections: 16,
        body_buffers: 4,
        ..ServerConfig::default()
    }
}

/// Starts a reader node over the compat blobs.
pub fn start() -> Node {
    start_with(store(), &MemoryBudget::new(&budget_config()))
}

pub fn budget_config() -> BudgetConfig {
    BudgetConfig::new(64 << 20)
        .with_pool(Pool::RequestBodies, 16 << 20)
        .expect("fits")
}

pub fn start_with(store: Arc<PooledStore<Emulator>>, budget: &MemoryBudget) -> Node {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let (server, service) = start_reader(
        listener,
        server_config(),
        Arc::clone(&store),
        "",
        NODE,
        budget,
    )
    .expect("starts");
    Node {
        server,
        store,
        service,
    }
}

/// Starts a node around a service built by `build`.
pub fn start_service(
    store: Arc<PooledStore<Emulator>>,
    build: impl FnOnce(SearchService<Emulator>) -> SearchService<Emulator>,
) -> Node {
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let service = Arc::new(build(SearchService::new(
        Arc::clone(&store),
        cache,
        "",
        NODE,
    )));
    let router = routes(Router::new(), Arc::clone(&service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&budget_config());
    let server = Server::start(listener, server_config(), router, &budget).expect("starts");
    Node {
        server,
        store,
        service,
    }
}

/// Puts the descriptor of `from` at `name`, with `edit` applied to its
/// JSON: the commit it names stays `from`'s.
pub fn put_variant(
    store: &PooledStore<Emulator>,
    from: &str,
    name: &str,
    edit: impl Fn(&str) -> String,
) {
    let path = blobs_root().join("indices").join(from);
    let bytes = std::fs::read(path).expect("descriptor");
    let (generation, json) = bytes.split_at(8);
    let mut out = generation.to_vec();
    out.extend_from_slice(edit(std::str::from_utf8(json).expect("utf-8")).as_bytes());
    store
        .put(&format!("indices/{name}"), &mut SliceSource::new(&out))
        .expect("put variant");
}

/// A reply.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Sends one request on its own connection and reads the reply to the end.
pub fn request(addr: SocketAddr, method: &str, target: &str, body: Option<&[u8]>) -> Reply {
    let mut s = TcpStream::connect(addr).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    let mut head = format!("{method} {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\n");
    if let Some(b) = body {
        head.push_str(&format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            b.len()
        ));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).expect("sends head");
    if let Some(b) = body {
        // A server that answers before the body ends closes its side; the
        // reply is still there to read.
        let _ = s.write_all(b);
    }
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 << 10];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    }
    parse(&raw)
}

fn parse(raw: &[u8]) -> Reply {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = std::str::from_utf8(&raw[..end]).expect("utf-8 head");
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .expect("a status");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let rest = &raw[end + 4..];
    let chunked = headers
        .iter()
        .any(|(n, v)| n == "transfer-encoding" && v.eq_ignore_ascii_case("chunked"));
    let body = if chunked {
        dechunk(rest)
    } else {
        rest.to_vec()
    };
    Reply {
        status,
        headers,
        body,
    }
}

fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .expect("a chunk size");
        let size = usize::from_str_radix(
            std::str::from_utf8(&rest[..line_end])
                .expect("utf-8")
                .trim(),
            16,
        )
        .expect("hex size");
        rest = &rest[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}
