//! Registers (docs/specs/registers/SPEC.md): one suite over the emulator,
//! the S3 client on loopback and the filesystem store, with exact request
//! counts per backend, then each backend's own cases.

mod basics;
mod counts;
mod faults;
mod fs;
mod races;
mod s3;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::emulator::http::S3Server;
use oss_blob::{
    BlobStore, Credentials, Emulator, FsStore, Meter, RetryPolicy, S3Config, S3Op, S3Store,
    SliceSource,
};
use oss_testkit::TempDir;

pub(crate) const BUCKET: &str = "test-bucket";
pub(crate) const REGION: &str = "us-east-1";

/// Which store a backend is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Emulator,
    S3,
    Fs,
}

/// A store under test, and what keeps it alive.
pub(crate) struct Backend {
    pub(crate) kind: Kind,
    pub(crate) store: Arc<dyn BlobStore>,
    pub(crate) server: Option<S3Server>,
    /// The emulator serving the requests, where there is one, for faults.
    pub(crate) emulator: Option<Arc<Emulator>>,
    _dir: Option<TempDir>,
}

pub(crate) fn credentials() -> Credentials {
    Credentials::new("test-access", "test-secret")
}

/// A client for `endpoint` with fast retries.
pub(crate) fn client(endpoint: &str) -> S3Store {
    let mut c = S3Config::new(endpoint, BUCKET, REGION, credentials());
    c.retry = RetryPolicy {
        max_attempts: 4,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(4),
    };
    c.jitter_seed = Some(1);
    c.io_timeout = Duration::from_secs(5);
    S3Store::new(c).unwrap()
}

pub(crate) fn emulator() -> Backend {
    let emulator = Arc::new(Emulator::new(64 << 20, 1));
    Backend {
        kind: Kind::Emulator,
        store: Arc::clone(&emulator) as Arc<dyn BlobStore>,
        server: None,
        emulator: Some(emulator),
        _dir: None,
    }
}

pub(crate) fn s3() -> Backend {
    let emulator = Arc::new(Emulator::new(64 << 20, 1));
    let server = S3Server::start(Arc::clone(&emulator), BUCKET, REGION, credentials()).unwrap();
    Backend {
        kind: Kind::S3,
        store: Arc::new(client(&server.endpoint())),
        server: Some(server),
        emulator: Some(emulator),
        _dir: None,
    }
}

pub(crate) fn fs() -> Backend {
    let dir = TempDir::new("registers");
    Backend {
        kind: Kind::Fs,
        store: Arc::new(FsStore::new(dir.path()).unwrap()),
        server: None,
        emulator: None,
        _dir: Some(dir),
    }
}

/// One of each backend.
pub(crate) fn all() -> [Backend; 3] {
    [emulator(), s3(), fs()]
}

/// The register's generation and value, or `None`.
pub(crate) fn read(store: &dyn BlobStore, key: &str) -> Option<(u64, Vec<u8>)> {
    let mut buf = [0u8; 1024];
    store
        .read_register(key, &mut buf)
        .unwrap()
        .map(|r| (r.generation, buf[..r.len].to_vec()))
}

/// Writes a register object with a plain PUT, as another writer would
/// without this store seeing it.
pub(crate) fn plant(store: &dyn BlobStore, key: &str, generation: u64, value: &[u8]) {
    let mut body = generation.to_be_bytes().to_vec();
    body.extend_from_slice(value);
    store
        .put(key, &mut SliceSource::new(&body), &mut [0u8; 4096])
        .unwrap();
}

/// The attempts of each kind `work` adds to `meter`.
pub(crate) fn counted<T>(meter: &Meter, work: impl FnOnce() -> T) -> (T, Vec<(S3Op, u64)>) {
    let before = S3Op::ALL.map(|op| meter.count(op));
    let out = work();
    let added = S3Op::ALL
        .iter()
        .zip(before)
        .map(|(op, b)| (*op, meter.count(*op).saturating_sub(b)))
        .filter(|(_, n)| *n > 0)
        .collect();
    (out, added)
}

/// Whether `g` is a first generation as the Java draws one.
pub(crate) fn is_initial(g: u64) -> bool {
    (1 << 32..1 << 62).contains(&g) && g & (1 << 32) != 0
}
