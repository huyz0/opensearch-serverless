//! Replays the search fixtures (fixtures/compat/search) against a reader
//! node over HTTP. The node cannot index, so each fixture's index comes
//! from the blobs the Java shell wrote for the same mapping and documents
//! (fixtures/compat-blobs), loaded into the in-process emulator; the steps
//! that create and fill an index, and the cleanup, are not sent. Every
//! `_search` step must match its recording after the fixture's own
//! normalisation.

// ARITH: test offsets and sizes are small; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_compat::pointer::Pointer;
use oss_compat::{
    Fixture, Request, Response, Rule, StepOutcome, Target, TargetError, load_dir, run_suite,
};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_rest::ServerConfig;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("blob dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(base, &path, out);
        } else if path.file_name().is_some_and(|n| n != "README.md") {
            let key = path
                .strip_prefix(base)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((key, std::fs::read(&path).expect("read blob")));
        }
    }
}

fn store() -> Arc<PooledStore<Emulator>> {
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 16).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let base = root().join("compat-blobs");
    let mut blobs = Vec::new();
    walk(&base, &base, &mut blobs);
    for (key, bytes) in &blobs {
        store
            .put(key, &mut SliceSource::new(bytes))
            .expect("load blob");
    }
    Arc::new(store)
}

/// A node over HTTP, one connection a request.
struct HttpTarget {
    addr: SocketAddr,
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Target for HttpTarget {
    fn send(&mut self, request: &Request) -> Result<Response, TargetError> {
        let failed = |e: std::io::Error| TargetError {
            message: e.to_string(),
        };
        let mut target = request.path.clone();
        for (i, (k, v)) in request.query.iter().enumerate() {
            target.push(if i == 0 { '?' } else { '&' });
            target.push_str(&format!("{}={}", encode(k), encode(v)));
        }
        let body = request.encode_body();
        let mut head = format!(
            "{} {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\n",
            request.method.as_str()
        );
        if let (Some(b), Some(ct)) = (&body, request.content_type()) {
            head.push_str(&format!(
                "content-type: {ct}\r\ncontent-length: {}\r\n",
                b.len()
            ));
        }
        head.push_str("\r\n");
        let mut s = TcpStream::connect(self.addr).map_err(failed)?;
        s.set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(failed)?;
        s.write_all(head.as_bytes()).map_err(failed)?;
        if let Some(b) = &body {
            s.write_all(b).map_err(failed)?;
        }
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(failed)?;
        Ok(parse(&raw))
    }
}

fn parse(raw: &[u8]) -> Response {
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
    let headers: std::collections::BTreeMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let mut rest = &raw[end + 4..];
    let body = if headers.get("transfer-encoding").map(String::as_str) == Some("chunked") {
        let mut out = Vec::new();
        loop {
            let line = rest
                .windows(2)
                .position(|w| w == b"\r\n")
                .expect("chunk size");
            let size =
                usize::from_str_radix(std::str::from_utf8(&rest[..line]).expect("utf-8"), 16)
                    .expect("hex size");
            rest = &rest[line + 2..];
            if size == 0 {
                break out;
            }
            out.extend_from_slice(&rest[..size]);
            rest = &rest[size + 2..];
        }
    } else {
        rest.to_vec()
    };
    let body = (!body.is_empty()).then(|| {
        serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("json body ({e}): {}", String::from_utf8_lossy(&body)))
    });
    Response {
        status,
        headers,
        body,
    }
}

/// The steps whose error names a column of the body. The fixture holds the
/// body as a JSON value, whose keys `oss-compat` keeps sorted, so the bytes
/// replayed are not the bytes recorded and the column moves. The reader's
/// own tests check these columns on the recorded bytes
/// (`dsl::source_tests` and `dsl::query_tests`).
const COLUMN_STEPS: &[(&str, &str)] = &[
    ("search errors", "unknown top-level key"),
    ("search errors", "unknown term parameter"),
];

/// The fixtures' `_search` steps alone, without their cleanup.
fn search_steps(fixtures: Vec<Fixture>) -> Vec<Fixture> {
    fixtures
        .into_iter()
        .map(|mut f| {
            f.cleanup.clear();
            f.steps.retain(|s| s.request.path.ends_with("/_search"));
            for step in &mut f.steps {
                let function_score = f.name == "match queries"
                    && step.name.starts_with("fs ")
                    && step
                        .response
                        .as_ref()
                        .and_then(|r| r.body.as_ref())
                        .is_some_and(|b| b.pointer("/error/col").is_some());
                if function_score || COLUMN_STEPS.contains(&(f.name.as_str(), step.name.as_str())) {
                    for at in ["/body/error/col", "/body/error/root_cause/0/col"] {
                        step.normalise.push(Rule {
                            pointer: Pointer::parse(at).expect("pointer"),
                            reason: "the replayed body's keys are re-sorted".into(),
                        });
                    }
                }
            }
            f
        })
        .collect()
}

#[test]
fn the_reader_node_answers_every_search_fixture_as_opensearch_did() {
    let fixtures: Vec<Fixture> = load_dir(&root().join("compat/search"))
        .expect("fixtures load")
        .into_iter()
        .map(|(_, f)| f)
        .collect();
    let fixtures = search_steps(fixtures);
    let steps: usize = fixtures.iter().map(|f| f.steps.len()).sum();
    assert!(steps >= 50, "the search fixtures hold {steps} searches");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, 16 << 20)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let config = ServerConfig {
        workers: 2,
        max_connections: 8,
        body_buffers: 2,
        ..ServerConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let (server, _) =
        oss_node::search::start_reader(listener, config, store(), "", "node-1", &budget)
            .expect("starts");
    let mut target = HttpTarget {
        addr: server.local_addr(),
    };
    let report = run_suite(&mut target, &fixtures);
    let failed: Vec<String> = report
        .fixtures
        .iter()
        .flat_map(|f| {
            f.steps
                .iter()
                .filter(|s| s.outcome != StepOutcome::Passed)
                .map(move |s| format!("{} / {}: {:?}", f.name, s.name, s.outcome))
        })
        .collect();
    server.shutdown();
    assert!(
        failed.is_empty(),
        "{} of {steps} searches differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
