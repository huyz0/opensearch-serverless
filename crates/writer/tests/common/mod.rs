//! Blobs and stores the node-log tests share.

#![allow(dead_code)]

pub mod committer;

use std::sync::Arc;

use oss_blob::register::{RegisterRead, SwapOutcome};
use oss_blob::{
    BlobResult, BlobStore, ByteRange, ChunkSink, ChunkSource, CreateOutcome, Emulator, GetOutcome,
    ListPage, Meter, ObjectMeta, S3Op,
};
use oss_format::node_log_format::encode;
use oss_format::{EncodedNodeLog, EncodedWalRecord, ShardBatch, ShardKey, WalRecord};
use oss_writer::log::NodeLogStore;

/// A record with a sequence number, as the Java tests make them.
pub fn record(id: &str, seq_no: i64) -> EncodedWalRecord {
    WalRecord::index(id, format!("{{\"n\":{seq_no}}}"), seq_no, 1, 1)
        .encode()
        .expect("encodes")
}

/// The ids of a slice's records.
pub fn ids(slice: &[u8]) -> Vec<String> {
    oss_format::slice_records(slice)
        .map(|r| r.expect("parses").id().to_owned())
        .collect()
}

/// A one-record blob for slot `ordinal` of epoch 0.
pub fn blob(ordinal: u64) -> Vec<u8> {
    let shard = ShardKey::new("uuid-a", 0, None::<&str>);
    let records = [record(
        &format!("r{ordinal}"),
        i64::try_from(ordinal).expect("fits"),
    )];
    encode(
        "node-1",
        0,
        ordinal,
        0,
        &[ShardBatch {
            shard: &shard,
            term: 1,
            records: &records,
        }],
        &[],
    )
    .expect("encodes")
    .bytes
}

/// Encodes one window.
pub fn window(ordinal: u64, batches: &[ShardBatch<'_>]) -> EncodedNodeLog {
    encode("node-1", 3, ordinal, 0, batches, &[]).expect("encodes")
}

/// A log on a fresh emulator.
pub fn emulated() -> (Arc<Emulator>, NodeLogStore) {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log = NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1")
        .expect("valid names");
    (store, log)
}

/// Requests of a kind since the meter was read last with `since`.
pub fn count(store: &Emulator, op: S3Op) -> u64 {
    store.meter().count(op)
}

/// What a [`Hooked`] store does in place of a create-if-absent.
pub type CreateHook = dyn Fn(&Emulator, &str, &mut dyn ChunkSource, &mut [u8]) -> BlobResult<CreateOutcome>
    + Send
    + Sync;

/// What a [`Hooked`] store does in place of a register swap.
pub type SwapHook = dyn Fn(&Emulator, &str, u64, &[u8]) -> BlobResult<SwapOutcome> + Send + Sync;

/// What a [`Hooked`] store does in place of a register read.
pub type ReadHook =
    dyn Fn(&Emulator, &str, &mut [u8]) -> BlobResult<Option<RegisterRead>> + Send + Sync;

/// An emulator whose creates go through a hook, and its register swaps and
/// reads through others when set; everything else is the emulator's.
pub struct Hooked {
    inner: Arc<Emulator>,
    create: Box<CreateHook>,
    swap: Option<Box<SwapHook>>,
    read: Option<Box<ReadHook>>,
}

impl Hooked {
    /// `inner` with its creates sent through `create`, as a store.
    pub fn store(
        inner: &Arc<Emulator>,
        create: impl Fn(&Emulator, &str, &mut dyn ChunkSource, &mut [u8]) -> BlobResult<CreateOutcome>
        + Send
        + Sync
        + 'static,
    ) -> Arc<dyn BlobStore> {
        Arc::new(Self {
            inner: Arc::clone(inner),
            create: Box::new(create),
            swap: None,
            read: None,
        })
    }

    /// `inner` with its register swaps sent through `swap`, as a store.
    pub fn swapping(
        inner: &Arc<Emulator>,
        swap: impl Fn(&Emulator, &str, u64, &[u8]) -> BlobResult<SwapOutcome> + Send + Sync + 'static,
    ) -> Arc<dyn BlobStore> {
        Arc::new(Self {
            inner: Arc::clone(inner),
            create: Box::new(|e, key, body, buf| e.create_if_absent(key, body, buf)),
            swap: Some(Box::new(swap)),
            read: None,
        })
    }

    /// `inner` with its register reads sent through `read`, as a store.
    pub fn reading(
        inner: &Arc<Emulator>,
        read: impl Fn(&Emulator, &str, &mut [u8]) -> BlobResult<Option<RegisterRead>>
        + Send
        + Sync
        + 'static,
    ) -> Arc<dyn BlobStore> {
        Arc::new(Self {
            inner: Arc::clone(inner),
            create: Box::new(|e, key, body, buf| e.create_if_absent(key, body, buf)),
            swap: None,
            read: Some(Box::new(read)),
        })
    }
}

impl BlobStore for Hooked {
    fn meter(&self) -> &Meter {
        self.inner.meter()
    }
    fn get(
        &self,
        key: &str,
        range: ByteRange,
        buf: &mut [u8],
        sink: &mut dyn ChunkSink,
    ) -> BlobResult<GetOutcome> {
        self.inner.get(key, range, buf, sink)
    }
    fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
        self.inner.put(key, body, buf)
    }
    fn create_if_absent(
        &self,
        key: &str,
        body: &mut dyn ChunkSource,
        buf: &mut [u8],
    ) -> BlobResult<CreateOutcome> {
        (self.create)(&self.inner, key, body, buf)
    }
    fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
        self.inner.head(key)
    }
    fn delete(&self, key: &str) -> BlobResult<()> {
        self.inner.delete(key)
    }
    fn delete_many(&self, keys: &[&str]) -> BlobResult<()> {
        self.inner.delete_many(keys)
    }
    // LIST: the trait's listing, delegated; the log never calls it.
    fn list_prefix(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
        visit: &mut dyn FnMut(&str, u64),
    ) -> BlobResult<ListPage> {
        // LIST: delegated, as above.
        self.inner.list_prefix(prefix, start_after, max_keys, visit)
    }
    fn read_register(&self, key: &str, buf: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
        match &self.read {
            Some(hook) => hook(&self.inner, key, buf),
            None => self.inner.read_register(key, buf),
        }
    }
    fn swap_register(&self, key: &str, expected: u64, value: &[u8]) -> BlobResult<SwapOutcome> {
        match &self.swap {
            Some(hook) => hook(&self.inner, key, expected, value),
            None => self.inner.swap_register(key, expected, value),
        }
    }
    fn create_register(&self, key: &str, value: &[u8]) -> BlobResult<SwapOutcome> {
        self.inner.create_register(key, value)
    }
    fn delete_register(&self, key: &str, expected: u64) -> BlobResult<bool> {
        self.inner.delete_register(key, expected)
    }
}
