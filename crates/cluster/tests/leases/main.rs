//! Node leases (docs/specs/node-leases/SPEC.md): renewal, revocation,
//! release, refresh, the digest and pruning, on the emulator and the
//! filesystem store with a simulated clock and exact request counts.

// ARITH: times and counts here are test constants and small sums; an
// overflow panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

mod budget;
mod early;
mod early_bounds;
mod java;
mod refresh;
mod renew;
mod revoke;
mod secret;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::{LeaseClock, LeaseConfig, Membership};
use oss_format::NodeLease;

/// The deployment's base inside the bucket.
pub(crate) const BASE: &str = "d/";

/// The Java's default lifetime.
pub(crate) const TTL: i64 = 30_000;

/// A start time well past 0, so an expiry of 0 is long dead.
pub(crate) const T0: i64 = 1_700_000_000_000;

/// A clock the test moves; pauses are counted, not slept.
#[derive(Default)]
pub(crate) struct TestClock {
    now: AtomicI64,
    pub(crate) pauses: AtomicU32,
    /// The pauses' total.
    pub(crate) paused_for: std::sync::Mutex<Duration>,
}

impl TestClock {
    pub(crate) fn at(now: i64) -> Arc<Self> {
        let clock = Self::default();
        clock.now.store(now, Ordering::SeqCst);
        Arc::new(clock)
    }

    pub(crate) fn advance(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
    }

    pub(crate) fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}

impl LeaseClock for TestClock {
    fn now_millis(&self) -> i64 {
        self.now()
    }

    fn pause(&self, pause: Duration) {
        self.pauses.fetch_add(1, Ordering::SeqCst);
        *self.paused_for.lock().unwrap() += pause;
    }
}

/// One store and clock, and the nodes on it.
pub(crate) struct Fleet {
    pub(crate) store: Arc<Emulator>,
    pub(crate) clock: Arc<TestClock>,
}

impl Fleet {
    pub(crate) fn new() -> Self {
        Self {
            store: Arc::new(Emulator::new(64 << 20, 1)),
            clock: TestClock::at(T0),
        }
    }

    pub(crate) fn config(&self, conditional_deletes: bool) -> LeaseConfig {
        LeaseConfig {
            ttl_millis: TTL,
            max_members: 16,
            conditional_deletes,
        }
    }

    /// A node's membership, probing the index on every young call.
    pub(crate) fn node(&self) -> Membership {
        self.node_with(self.config(false))
    }

    pub(crate) fn node_with(&self, config: LeaseConfig) -> Membership {
        let store: Arc<dyn BlobStore> = self.store.clone();
        let clock: Arc<dyn LeaseClock> = self.clock.clone();
        Membership::new(store, BASE, clock, config)
            .unwrap()
            .with_probe_interval(Duration::ZERO)
    }

    /// Requests of `op` so far.
    pub(crate) fn count(&self, op: S3Op) -> u64 {
        self.store.meter().count(op)
    }

    /// Every request so far.
    pub(crate) fn total(&self) -> u64 {
        self.store.meter().total()
    }

    /// The register at `key` under the base, read raw.
    pub(crate) fn read(&self, key: &str) -> Option<(u64, Vec<u8>)> {
        let mut buf = vec![0u8; 1 << 16];
        let read = self
            .store
            .read_register(&format!("{BASE}{key}"), &mut buf)
            .unwrap()?;
        buf.truncate(read.len);
        Some((read.generation, buf))
    }

    /// The lease stored for `node_id`.
    pub(crate) fn lease(&self, node_id: &str) -> Option<NodeLease> {
        self.read(&format!("cluster/members/lease-{node_id}"))
            .map(|(_, bytes)| NodeLease::parse(&bytes).unwrap())
    }

    /// The ids the members index names.
    pub(crate) fn index(&self) -> Vec<String> {
        self.read("cluster/members/members")
            .map(|(_, bytes)| oss_format::decode_members(&bytes).unwrap())
            .unwrap_or_default()
    }
}

/// A lease for `id`, process `e<id>`, at address `a<id>`.
pub(crate) fn lease(id: &str) -> NodeLease {
    let mut lease = NodeLease::new(id, &format!("e{id}"), &format!("a{id}"), 0);
    lease.set_roles(["search".to_owned()]);
    lease
}

/// The node ids of `leases`, in order.
pub(crate) fn ids(leases: &[NodeLease]) -> Vec<&str> {
    leases.iter().map(|l| l.node_id.as_str()).collect()
}

/// A store that runs `hook` before each swap or create of a register, so a
/// test can put a competing write between a node's read and its swap.
pub(crate) struct Racing {
    pub(crate) inner: Arc<Emulator>,
    #[allow(clippy::type_complexity)]
    pub(crate) hook: std::sync::Mutex<Option<Box<dyn FnMut(&str) + Send>>>,
    /// Runs before each register read, outside any lock.
    #[allow(clippy::type_complexity)]
    pub(crate) read_hook: std::sync::Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

impl Racing {
    pub(crate) fn new(inner: Arc<Emulator>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            hook: std::sync::Mutex::new(None),
            read_hook: std::sync::Mutex::new(None),
        })
    }

    pub(crate) fn set(&self, hook: impl FnMut(&str) + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(hook));
    }

    fn before(&self, key: &str) {
        // Taken out while it runs, so a hook that writes is not hooked.
        let hook = self.hook.lock().unwrap().take();
        if let Some(mut hook) = hook {
            hook(key);
            let mut slot = self.hook.lock().unwrap();
            if slot.is_none() {
                *slot = Some(hook);
            }
        }
    }
}

impl BlobStore for Racing {
    fn meter(&self) -> &oss_blob::Meter {
        self.inner.meter()
    }
    fn get(
        &self,
        key: &str,
        range: oss_blob::ByteRange,
        buf: &mut [u8],
        sink: &mut dyn oss_blob::ChunkSink,
    ) -> oss_blob::BlobResult<oss_blob::GetOutcome> {
        self.inner.get(key, range, buf, sink)
    }
    fn put(
        &self,
        key: &str,
        body: &mut dyn oss_blob::ChunkSource,
        buf: &mut [u8],
    ) -> oss_blob::BlobResult<()> {
        self.inner.put(key, body, buf)
    }
    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn oss_blob::ChunkSource,
        buf: &mut [u8],
    ) -> oss_blob::BlobResult<oss_blob::CreateOutcome> {
        self.inner.create_if_absent(key, body, buf)
    }
    fn head(&self, key: &str) -> oss_blob::BlobResult<Option<oss_blob::ObjectMeta>> {
        self.inner.head(key)
    }
    fn delete(&self, key: &str) -> oss_blob::BlobResult<()> {
        self.inner.delete(key)
    }
    // LIST: forwarded only; nothing in these tests lists.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> oss_blob::BlobResult<oss_blob::ListPage> {
        // LIST: forwarded to the wrapped store unchanged.
        self.inner.list_prefix(prefix, start_after, max_keys, visit)
    }
    fn read_register(
        &self,
        key: &str,
        buf: &mut [u8],
    ) -> oss_blob::BlobResult<Option<oss_blob::RegisterRead>> {
        let hook = self.read_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(key);
        }
        self.inner.read_register(key, buf)
    }
    fn swap_register(
        &self,
        key: &str,
        expected: u64,
        value: &[u8],
    ) -> oss_blob::BlobResult<oss_blob::SwapOutcome> {
        self.before(key);
        self.inner.swap_register(key, expected, value)
    }
    fn create_register(
        &self,
        key: &str,
        value: &[u8],
    ) -> oss_blob::BlobResult<oss_blob::SwapOutcome> {
        self.before(key);
        self.inner.create_register(key, value)
    }
    fn delete_register(&self, key: &str, expected: u64) -> oss_blob::BlobResult<bool> {
        self.inner.delete_register(key, expected)
    }
}

impl Fleet {
    /// A node over `store` instead of the fleet's own.
    pub(crate) fn node_on(&self, store: Arc<dyn BlobStore>) -> Membership {
        let clock: Arc<dyn LeaseClock> = self.clock.clone();
        Membership::new(store, BASE, clock, self.config(false))
            .unwrap()
            .with_probe_interval(Duration::ZERO)
    }
}
