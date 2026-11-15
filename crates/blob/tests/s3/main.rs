//! The S3 client (docs/specs/s3-client/SPEC.md): the signer against AWS's
//! published vectors, and the client against the emulator's S3 HTTP front
//! end on loopback. Every test asserts exact request counts, on the
//! client's meter and on the server.

mod gate;
mod listing;
mod load;
mod protocol;
mod reads;
mod retries;
mod server;
mod sigv4;
mod writes;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::emulator::http::S3Server;
use oss_blob::{
    BlobResult, BlobStore, ByteRange, Credentials, Emulator, Meter, RetryPolicy, S3Config, S3Op,
    S3Store, SliceSource,
};

pub(crate) const MIB: u64 = 1 << 20;
pub(crate) const BUCKET: &str = "test-bucket";
pub(crate) const REGION: &str = "us-east-1";

pub(crate) fn credentials() -> Credentials {
    Credentials::new("test-access", "test-secret")
}

/// A client with fast retries for `endpoint`.
pub(crate) fn config(endpoint: &str, credentials: Credentials) -> S3Config {
    let mut c = S3Config::new(endpoint, BUCKET, REGION, credentials);
    c.retry = RetryPolicy {
        max_attempts: 4,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(4),
    };
    c.jitter_seed = Some(1);
    // A response the server never finishes fails a test in seconds.
    c.io_timeout = Duration::from_secs(5);
    c
}

/// A server over a fresh emulator, and a client for it.
pub(crate) struct Fixture {
    pub(crate) server: S3Server,
    pub(crate) client: S3Store,
}

pub(crate) fn fixture() -> Fixture {
    let store = Arc::new(Emulator::new(64 * MIB, 1));
    let server = S3Server::start(store, BUCKET, REGION, credentials()).unwrap();
    let client = S3Store::new(config(&server.endpoint(), credentials())).unwrap();
    Fixture { server, client }
}

/// Asserts the meter holds exactly `expected` attempts and nothing else.
pub(crate) fn assert_counts(meter: &Meter, expected: &[(S3Op, u64)]) {
    for op in S3Op::ALL {
        let want = expected
            .iter()
            .find(|(o, _)| *o == op)
            .map_or(0, |(_, n)| *n);
        assert_eq!(meter.count(op), want, "{op:?}");
    }
}

pub(crate) fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    store
        .put(key, &mut SliceSource::new(bytes), &mut [0u8; 4096])
        .unwrap();
}

pub(crate) fn get_all(store: &dyn BlobStore, key: &str, range: ByteRange) -> BlobResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut sink = |c: &[u8]| {
        out.extend_from_slice(c);
        Ok(())
    };
    store.get(key, range, &mut [0u8; 1000], &mut sink)?;
    Ok(out)
}

pub(crate) fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}
