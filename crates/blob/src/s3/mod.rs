//! The S3 client: a blocking [`BlobStore`] over S3's REST API.
//!
//! ADR 0005 decision 3: HTTP is `ureq` with rustls, requests are signed
//! with SigV4 written here ([`sigv4`]), addressing is path-style
//! (`scheme://host/bucket/key`), and the retry policy ([`retry`]) is ours,
//! so the code that sends each attempt is the code that records it in the
//! [`Meter`]. Bodies stream through the caller's buffer: a GET reads the
//! response into it chunk by chunk, a PUT fills it from the source and
//! sends it with a declared `Content-Length` and an `UNSIGNED-PAYLOAD`
//! hash. Nothing here holds a whole object.
//!
//! The Java shell does the same through the AWS SDK in `S3BlobContainer`
//! (`plugins/repository-s3/src/main/java/org/opensearch/repositories/s3/S3BlobContainer.java@49f238ed`);
//! its mapping of a refused conditional write (412, or 409
//! `PreconditionFailed`) against a retryable 409 is ported in
//! `request.rs`.

mod body;
pub(crate) mod delete;
mod register;
mod request;
pub mod retry;
pub mod sigv4;
pub(crate) mod xml;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::chunk::{ChunkSink, ChunkSource};
use crate::error::{BlobError, BlobResult};
use crate::meter::Meter;
use crate::register::known::KnownVersions;
use crate::register::{RegisterRead, SwapOutcome};
use crate::store::{
    BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, ObjectMeta, check_buf, check_key,
    check_list,
};
use request::{Condition, PutAnswer};
use retry::{Jitter, RetryPolicy, retryable};
use sigv4::Credentials;

/// Where the store is and how the client talks to it.
#[derive(Clone, Debug)]
pub struct S3Config {
    /// `http://host:port` or `https://host`, with no path. Plain `http` is
    /// for loopback tests: bodies are signed `UNSIGNED-PAYLOAD`, so only
    /// TLS protects them.
    pub endpoint: String,
    /// The bucket, addressed path-style.
    pub bucket: String,
    /// The region requests are signed for.
    pub region: String,
    /// The key that signs every request.
    pub credentials: Credentials,
    /// Attempts and backoff per call.
    pub retry: RetryPolicy,
    /// How long a connection may take to open.
    pub connect_timeout: Duration,
    /// How long one read or write on a connection may stall.
    pub io_timeout: Duration,
    /// Idle connections kept open for reuse.
    pub max_idle_connections: usize,
    /// Bytes of each of the two buffers `ureq` holds per connection; a
    /// response's headers must fit in one.
    pub connection_buffer: usize,
    /// Seeds the retry jitter; `None` seeds it from the clock.
    pub jitter_seed: Option<u64>,
}

impl S3Config {
    /// A configuration with the default retry policy, 2 s to connect, 30 s
    /// per stalled read or write, 64 idle connections and 16 KiB connection
    /// buffers.
    pub fn new(endpoint: &str, bucket: &str, region: &str, credentials: Credentials) -> Self {
        Self {
            // ALLOC: once per store, at construction.
            endpoint: endpoint.to_owned(),
            // ALLOC: once per store, at construction.
            bucket: bucket.to_owned(),
            // ALLOC: once per store, at construction.
            region: region.to_owned(),
            credentials,
            retry: RetryPolicy::default(),
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(30),
            max_idle_connections: 64,
            connection_buffer: 16 * 1024,
            jitter_seed: None,
        }
    }
}

/// A [`BlobStore`] on S3, or on anything that speaks its REST API.
#[derive(Debug)]
pub struct S3Store {
    agent: ureq::Agent,
    /// `scheme://authority`, the URL every request starts with.
    origin: String,
    /// The `Host` header value the signature covers.
    host: String,
    bucket: String,
    region: String,
    credentials: Credentials,
    retry: RetryPolicy,
    jitter: Jitter,
    /// The random half of this store's writer tokens.
    writer_base: u64,
    meter: Meter,
    /// The register versions this store last read or wrote.
    known: KnownVersions,
}

/// Writer tokens issued in this process, the counting half of each.
static WRITERS: AtomicU64 = AtomicU64::new(0);

/// Whether `bucket` is a name S3 accepts: 3 to 63 lowercase letters,
/// digits, dots and hyphens.
fn valid_bucket(bucket: &str) -> bool {
    (3..=63).contains(&bucket.len())
        && bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

impl S3Store {
    /// A client for `config`'s bucket. Fails with
    /// [`BlobError::InvalidArgument`] when the endpoint is not
    /// `http(s)://authority`, the bucket is not a valid name, or the policy
    /// allows no attempt. Sends nothing.
    pub fn new(config: S3Config) -> BlobResult<Self> {
        let (host, default_port) = match config.endpoint.strip_prefix("https://") {
            Some(host) => (host, ":443"),
            None => (
                config
                    .endpoint
                    .strip_prefix("http://")
                    .ok_or(BlobError::InvalidArgument(
                        "endpoint is not http:// or https://",
                    ))?,
                ":80",
            ),
        };
        if host.is_empty() || host.contains(['/', '?', '#', '@']) {
            return Err(BlobError::InvalidArgument(
                "endpoint has no host or has a path",
            ));
        }
        if !valid_bucket(&config.bucket) {
            return Err(BlobError::InvalidArgument("invalid bucket name"));
        }
        if config.retry.max_attempts == 0 {
            return Err(BlobError::InvalidArgument("max_attempts of 0"));
        }
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                // Every status is the client's to interpret.
                .http_status_as_error(false)
                // A redirect would send a request no attempt records.
                .max_redirects(0)
                // The endpoint is reached directly, whatever the environment says.
                .proxy(None)
                .timeout_connect(Some(config.connect_timeout))
                .timeout_per_read(Some(config.io_timeout))
                .timeout_per_write(Some(config.io_timeout))
                .max_idle_connections(config.max_idle_connections)
                .max_idle_connections_per_host(config.max_idle_connections)
                .input_buffer_size(config.connection_buffer)
                .output_buffer_size(config.connection_buffer)
                .build(),
        );
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        let seed = config.jitter_seed.unwrap_or(now);
        // Never from the jitter seed: two stores seeded alike must still
        // issue different tokens, in one process or in two.
        let writer_base = Jitter::new(
            now ^ u64::from(std::process::id()).rotate_left(32)
                ^ WRITERS.fetch_add(1, Ordering::Relaxed).rotate_left(16),
        )
        .roll();
        Ok(Self {
            agent,
            // The scheme's default port is left out, as HTTP clients write
            // the Host header; the signed value and the sent one must match.
            // ALLOC: once per store, at construction.
            host: host.strip_suffix(default_port).unwrap_or(host).to_owned(),
            origin: config.endpoint.clone(),
            bucket: config.bucket,
            region: config.region,
            credentials: config.credentials,
            retry: config.retry,
            jitter: Jitter::new(seed),
            writer_base,
            meter: Meter::new(),
            known: KnownVersions::default(),
        })
    }

    /// Runs `once` until it succeeds, fails for good, or the policy's
    /// attempts run out, pausing with jitter between attempts. `once` gets
    /// the attempt number, records its own request, and answers whether
    /// its failure may be repeated; it answers `None` when it cannot even
    /// start again (a body that does not rewind). Each failed attempt is
    /// counted once as an error.
    fn run<T>(&self, mut once: impl FnMut(u32) -> Option<(BlobResult<T>, bool)>) -> BlobResult<T> {
        let mut last = BlobError::Unavailable;
        for attempt in 0..self.retry.max_attempts {
            if let Some(retry) = attempt.checked_sub(1) {
                std::thread::sleep(self.retry.pause(retry, self.jitter.roll()));
            }
            let Some((result, repeatable)) = once(attempt) else {
                break;
            };
            match result {
                Ok(value) => return Ok(value),
                Err(err) => {
                    self.meter.record_error();
                    let again = repeatable && retryable(&err);
                    last = err;
                    if !again {
                        break;
                    }
                }
            }
        }
        Err(last)
    }

    fn write(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
        cond: Option<Condition<'_>>,
        last_attempt: &mut u32,
    ) -> BlobResult<PutAnswer> {
        check_key(key)?;
        check_buf(buf)?;
        // An empty body sends no bytes, so its source is checked here: one
        // that yields any is refused before a request goes out.
        if body.is_empty() && body.fill(buf)? != 0 {
            return Err(BlobError::InvalidArgument(
                "body longer than its declared length",
            ));
        }
        self.run(|attempt| {
            if attempt > 0 && body.rewind().is_err() {
                return None;
            }
            *last_attempt = attempt;
            Some(self.put_once(key, body, buf, cond))
        })
    }

    /// A token no other conditional write carries: this store's random half and the
    /// process's count of tokens issued.
    fn writer_token(&self) -> String {
        let n = WRITERS.fetch_add(1, Ordering::Relaxed);
        // ALLOC: one 32-byte token per conditional write.
        format!("{:016x}{n:016x}", self.writer_base)
    }
}

impl BlobStore for S3Store {
    fn meter(&self) -> &Meter {
        &self.meter
    }

    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        check_key(key)?;
        check_buf(buf)?;
        if let ByteRange::From { len: 0, .. } = range {
            return Err(BlobError::InvalidArgument("range of 0 bytes"));
        }
        self.run(|_| Some(self.get_once(key, range, buf, sink)))
            .map(|(outcome, _)| outcome)
    }

    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.write(key, body, buf, None, &mut 0).map(|_| ())
    }

    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        crate::register::tagged::read(self, key, buf)
    }

    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        crate::register::tagged::swap(self, key, expected, value)
    }

    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        crate::register::tagged::create(self, key, value)
    }

    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        crate::register::tagged::delete(self, key, expected)
    }

    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        let pre = crate::register::tagged::Precondition::Absent;
        Ok(match self.put_if(key, body, buf, pre)? {
            Some(_) => CreateOutcome::Created,
            None => CreateOutcome::AlreadyExists,
        })
    }

    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        check_key(key)?;
        self.run(|_| {
            Some((
                self.head_once(key, "").map(|h| h.map(|(meta, _, _)| meta)),
                true,
            ))
        })
    }

    fn delete(&self, key: &str) -> BlobResult<()> {
        check_key(key)?;
        self.run(|_| Some((self.delete_once(key, None).map(|_| ()), true)))
    }

    fn delete_many(&self, keys: &[&str]) -> BlobResult<()> {
        self.delete_batches(keys)
    }

    // LIST: the client's implementation of the trait's one bounded page.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        let max = check_list(prefix, start_after, max_keys)?;
        self.run(|_| Some(self.list_once(prefix, start_after, max, visit)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str, bucket: &str) -> S3Config {
        S3Config::new(endpoint, bucket, "us-east-1", Credentials::new("a", "s"))
    }

    #[test]
    fn stores_seeded_alike_issue_different_writer_tokens() {
        let mut c = config("http://h:1", "bucket");
        c.jitter_seed = Some(1);
        let a = S3Store::new(c.clone()).unwrap();
        let b = S3Store::new(c).unwrap();
        assert_ne!(a.writer_base, b.writer_base);
        let (t1, t2) = (a.writer_token(), a.writer_token());
        assert_eq!(t1.len(), 32);
        assert_ne!(t1, t2);
        assert_eq!(t1[..16], t2[..16]);
    }

    #[test]
    fn endpoints_and_buckets_are_checked_before_anything_is_sent() {
        for (endpoint, bucket, why) in [
            ("ftp://h", "bucket", "endpoint is not http:// or https://"),
            ("http://", "bucket", "endpoint has no host or has a path"),
            ("http://h/p", "bucket", "endpoint has no host or has a path"),
            ("https://h", "Bucket", "invalid bucket name"),
            ("https://h", "bu", "invalid bucket name"),
        ] {
            assert_eq!(
                S3Store::new(config(endpoint, bucket)).map(|_| ()),
                Err(BlobError::InvalidArgument(why)),
                "{endpoint} {bucket}"
            );
        }
        let mut none = config("http://h:1", "bucket");
        none.retry.max_attempts = 0;
        assert_eq!(
            S3Store::new(none).map(|_| ()),
            Err(BlobError::InvalidArgument("max_attempts of 0"))
        );
        let store = S3Store::new(config("https://h:9000", "my.bucket-1")).unwrap();
        assert_eq!(store.host, "h:9000");
        for (endpoint, host) in [
            ("https://h:443", "h"),
            ("http://h:80", "h"),
            ("http://h:443", "h:443"),
            ("https://h:80", "h:80"),
        ] {
            assert_eq!(S3Store::new(config(endpoint, "bucket")).unwrap().host, host);
        }
        assert_eq!(store.meter().total(), 0);
    }
}
