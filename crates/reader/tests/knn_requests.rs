//! The object-store requests a `knn` search makes over vectors of a
//! realistic size, against the same index's term search, through the
//! in-process emulator (which meters every call as the S3 client makes it).
//! Not pass/fail: it prints what `docs/knn-measurements.md` records, and is
//! ignored so `cargo test` does not run it.
//!
//! ```text
//! KNN_DOCS=100000 KNN_DIM=128 KNN_SEGMENT=10000 \
//!   cargo test -p oss-reader --release --test knn_requests -- --ignored --nocapture
//! ```

// ARITH: measurement code over small counts.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    clippy::print_stdout
)]

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::VectorSimilarityFunction;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::s3::sigv4::{self, Credentials, Scope, UNSIGNED_PAYLOAD};
use oss_blob::{BlobStore, Emulator, PooledStore, S3Config, S3Op, S3Store, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::parse_chunks;
use oss_reader::open_shard;
use oss_reader::search::search;
use oss_testkit::index::{Spec, blobs, spec_vector};

static BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn counts<S: BlobStore>(store: &PooledStore<S>) -> [u64; 7] {
    S3Op::ALL.map(|op| store.store().meter().count(op))
}

fn since<S: BlobStore>(store: &PooledStore<S>, before: [u64; 7]) -> String {
    let now = counts(store);
    let bytes = store.store().meter().bytes_read();
    let delta = bytes - BYTES.swap(bytes, std::sync::atomic::Ordering::Relaxed);
    let head = format!("bytes_read={delta} ");
    head + &S3Op::ALL
        .iter()
        .zip(now.iter().zip(before.iter()))
        .filter(|(_, (n, b))| n > b)
        .map(|(op, (n, b))| format!("{op:?}={}", n - b))
        .collect::<Vec<_>>()
        .join(" ")
}

const ACCESS: &str = "rustfsadmin";
const BUCKET: &str = "knn-requests";
const REGION: &str = "us-east-1";

/// A RustFS process on loopback, killed when the measurement ends.
struct RustFs(std::process::Child);

impl Drop for RustFs {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts the RustFS binary `bin` and creates the bucket, as
/// `crates/blob/tests/rustfs.rs` does; returns the process and the store.
fn start_rustfs(bin: &std::ffi::OsStr) -> (RustFs, S3Store) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("port")
        .local_addr()
        .expect("addr")
        .port();
    let dir = std::env::temp_dir().join(format!("oss-knn-rustfs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("data dir");
    let host = format!("127.0.0.1:{port}");
    let process = RustFs(
        std::process::Command::new(bin)
            .arg(&dir)
            .env("RUSTFS_ADDRESS", &host)
            .env("RUSTFS_VOLUMES", &dir)
            .env("RUSTFS_ACCESS_KEY", ACCESS)
            .env("RUSTFS_SECRET_KEY", ACCESS)
            .env("RUSTFS_CONSOLE_ENABLE", "false")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("RUSTFS_BIN starts"),
    );
    let started = std::time::Instant::now();
    while std::net::TcpStream::connect(&host).is_err() {
        assert!(started.elapsed().as_secs() < 30, "RustFS did not listen");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let endpoint = format!("http://{host}");
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .build(),
    );
    // The first create can land before the store accepts writes: retry.
    for attempt in 0.. {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let date = sigv4::amz_date(now);
        let headers = [
            ("host", host.as_str()),
            ("x-amz-content-sha256", UNSIGNED_PAYLOAD),
            ("x-amz-date", date.as_str()),
        ];
        let path = format!("/{BUCKET}");
        let canonical = sigv4::canonical_request("PUT", &path, "", &headers, UNSIGNED_PAYLOAD);
        let scope = Scope {
            date: &date[..8],
            region: REGION,
            service: "s3",
        };
        let auth =
            sigv4::authorization(&Credentials::new(ACCESS, ACCESS), &scope, &date, &canonical);
        let status = agent
            .put(&format!("{endpoint}{path}"))
            .header("x-amz-content-sha256", UNSIGNED_PAYLOAD)
            .header("x-amz-date", &date)
            .header("authorization", &auth)
            .send_empty()
            .map(|r| r.status().as_u16())
            .unwrap_or(0);
        if status == 200 {
            break;
        }
        assert!(attempt < 20, "create bucket answered {status}");
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let store = S3Store::new(S3Config::new(
        &endpoint,
        BUCKET,
        REGION,
        Credentials::new(ACCESS, ACCESS),
    ))
    .expect("store");
    (process, store)
}

#[test]
#[ignore = "a measurement, run by hand"]
fn requests() {
    // `WARM_AT_OPEN=1` reads every vector file at open, as the engine does by
    // default; `0` (the default here) leaves that to the first `knn` search.
    oss_reader::vector_warm::set_at_open(std::env::var("WARM_AT_OPEN").is_ok_and(|v| v == "1"));
    let docs = env("KNN_DOCS", 100_000);
    let dim = env("KNN_DIM", 128);
    let segment = env("KNN_SEGMENT", 10_000);
    let spec = Spec::new("kn", docs)
        .segments_of(segment)
        .with_vectors(VectorSimilarityFunction::Euclidean)
        .with_vector_dimension(dim);
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    // `RUSTFS_BIN` names a RustFS binary: the blobs go to it. Otherwise the
    // in-process emulator holds them.
    if let Some(bin) = std::env::var_os("RUSTFS_BIN") {
        println!("store: RustFS");
        let (_process, s3) = start_rustfs(&bin);
        measure(Arc::new(PooledStore::new(s3, buffers)), &spec);
    } else {
        println!("store: emulator");
        measure(
            Arc::new(PooledStore::new(Emulator::new(1 << 30, 1), buffers)),
            &spec,
        );
    }
}

fn measure<S: BlobStore + 'static>(store: Arc<PooledStore<S>>, spec: &Spec) {
    let (docs, dim, segment) = (spec.docs, spec.vector_dimension, spec.docs_per_segment);
    let mut bytes = 0usize;
    for (key, blob) in blobs("", spec) {
        bytes += blob.len();
        store.put(&key, &mut SliceSource::new(&blob)).expect("put");
    }
    let vector: Vec<String> = spec_vector(spec, 1)
        .expect("vector")
        .iter()
        .map(f32::to_string)
        .collect();
    let knn = format!(
        r#"{{"query":{{"knn":{{"v":{{"vector":[{}],"k":10}}}}}},"size":10}}"#,
        vector.join(",")
    );
    let term = r#"{"query":{"term":{"tag":"t3"}},"size":10}"#.to_owned();
    println!(
        "config: docs={docs} dim={dim} segment={segment} index bytes={}",
        bytes
    );
    println!(
        "demand paging available on this host: {}",
        lucene_store::page_cache::demand_paging_available()
    );
    let analyzer = Analyzer::standard(None);
    for (label, body) in [("term", &term), ("knn", &knn)] {
        // Each row opens the shard with an empty page cache: a cold node.
        let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
        let before = counts(&store);
        let shard = open_shard(store.clone(), cache, "", "kn", 0).expect("opens");
        println!("{label}: open: {}", since(&store, before));
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        let before = counts(&store);
        let hits = search(&shard, "kn", &source, &analyzer).expect("search");
        println!(
            "{label}: first search ({} hits): {}",
            hits.hits.len(),
            since(&store, before)
        );
        let before = counts(&store);
        search(&shard, "kn", &source, &analyzer).expect("search");
        println!("{label}: second search: {}", since(&store, before));
    }
}
