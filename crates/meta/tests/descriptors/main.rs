//! The descriptor store (docs/specs/descriptor-store/SPEC.md): one suite
//! over the emulator, the S3 client on loopback and the filesystem store,
//! with exact request counts per backend.

// ARITH: generations, times and counts here are test constants and small
// sums; an overflow panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

mod alias_mutations;
mod alias_races;
mod aliases;
mod budgets;
mod changes;
mod cluster_config;
mod counts;
mod doc_cache;
mod doc_sets;
mod index_lifecycle;
mod lifecycle;
mod listing;
mod point_in_time;
mod point_in_time_bucket;
mod point_in_time_capture;
mod point_in_time_races;
mod probe;
mod reclaim;
mod reclaim_modes;
mod repositories;
mod routing;
mod snapshot_capture;
mod snapshot_remove;
mod snapshot_restore;
mod snapshot_take;
mod snapshot_take_faults;
mod snapshots;
mod sweep;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::emulator::http::S3Server;
use oss_blob::{
    BlobStore, Credentials, Emulator, FsStore, Meter, RetryPolicy, S3Config, S3Op, S3Store,
};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_testkit::TempDir;

const BUCKET: &str = "test-bucket";
const REGION: &str = "us-east-1";

/// The deployment's base inside the bucket.
pub(crate) const BASE: &str = "d/";

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
    _server: Option<S3Server>,
    _dir: Option<TempDir>,
}

impl Backend {
    /// A descriptor store over this backend.
    pub(crate) fn descriptors(&self, deletes: DeleteMode) -> DescriptorStore {
        DescriptorStore::new(Arc::clone(&self.store), BASE, deletes).unwrap()
    }

    /// The meter the store records in.
    pub(crate) fn meter(&self) -> &Meter {
        self.store.meter()
    }

    /// Picks the object stores' count or the filesystem's.
    pub(crate) fn budget(&self, object: &[(S3Op, u64)], file: &[(S3Op, u64)]) -> Vec<(S3Op, u64)> {
        if self.kind == Kind::Fs {
            file.to_vec()
        } else {
            object.to_vec()
        }
    }
}

fn emulator() -> Backend {
    Backend {
        kind: Kind::Emulator,
        store: Arc::new(Emulator::new(64 << 20, 1)),
        _server: None,
        _dir: None,
    }
}

fn s3() -> Backend {
    let credentials = Credentials::new("test-access", "test-secret");
    let emulator = Arc::new(Emulator::new(64 << 20, 1));
    let server = S3Server::start(emulator, BUCKET, REGION, credentials.clone()).unwrap();
    let mut config = S3Config::new(&server.endpoint(), BUCKET, REGION, credentials);
    config.retry = RetryPolicy {
        max_attempts: 4,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(4),
    };
    config.jitter_seed = Some(1);
    config.io_timeout = Duration::from_secs(5);
    Backend {
        kind: Kind::S3,
        store: Arc::new(S3Store::new(config).unwrap()),
        _server: Some(server),
        _dir: None,
    }
}

fn fs() -> Backend {
    let dir = TempDir::new("descriptors");
    Backend {
        kind: Kind::Fs,
        store: Arc::new(FsStore::new(dir.path()).unwrap()),
        _server: None,
        _dir: Some(dir),
    }
}

/// One of each backend.
pub(crate) fn all() -> [Backend; 3] {
    [emulator(), s3(), fs()]
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
