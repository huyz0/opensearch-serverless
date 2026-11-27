//! docs/specs/stock-snapshot-export AC-6: a fault at every write of a take
//! leaves a repository whose `index.latest` still names a restorable
//! generation, and the writes are ordered so it does.

mod stock_export_support;

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, ExportStore};
use oss_meta::{
    ExportError, ExportIndex, ExportSnapshot, ExportTarget, StockRepo, open_export_base,
    publish_snapshot,
};
use stock_export_support::{PREFIX, Take, first, publish, shards, store};

mod faults {
    use std::sync::Mutex;

    use oss_blob::{
        BlobError, BlobResult, BlobStore, ByteRange, ChunkSink, ChunkSource, CreateOutcome,
        GetOutcome, ListPage, Meter, ObjectMeta, RegisterRead, SwapOutcome,
    };

    /// A store that logs every write in order and fails the one at `fail_at`.
    pub struct Recording<S> {
        pub inner: S,
        pub log: Mutex<Vec<String>>,
        pub fail_at: Option<usize>,
        /// How many `get`s have been made, and the one to fail.
        pub reads: Mutex<usize>,
        pub fail_read_at: Option<usize>,
    }

    impl<S: BlobStore> Recording<S> {
        fn write(&self, what: &str, key: &str) -> BlobResult<()> {
            let mut log = self.log.lock().unwrap();
            let n = log.len();
            log.push(format!("{what} {key}"));
            if self.fail_at == Some(n) {
                return Err(BlobError::Unavailable);
            }
            Ok(())
        }
    }

    impl<S: BlobStore> BlobStore for Recording<S> {
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
            let mut reads = self.reads.lock().unwrap();
            let n = *reads;
            *reads += 1;
            drop(reads);
            if self.fail_read_at == Some(n) {
                return Err(BlobError::Unavailable);
            }
            self.inner.get(key, range, buf, sink)
        }
        fn put(&self, key: &str, body: &mut dyn ChunkSource, buf: &mut [u8]) -> BlobResult<()> {
            self.write("put", key)?;
            self.inner.put(key, body, buf)
        }
        fn create_if_absent(
            &self,
            key: &str,
            body: &mut dyn ChunkSource,
            buf: &mut [u8],
        ) -> BlobResult<CreateOutcome> {
            self.write("create", key)?;
            self.inner.create_if_absent(key, body, buf)
        }
        fn head(&self, key: &str) -> BlobResult<Option<ObjectMeta>> {
            self.inner.head(key)
        }
        fn delete(&self, key: &str) -> BlobResult<()> {
            self.inner.delete(key)
        }
        // LIST: refused, not issued: a take lists nothing.
        fn list_prefix(
            &self,
            _prefix: &str,
            _start_after: Option<&str>,
            _max_keys: usize,
            _visit: &mut dyn FnMut(&str, u64),
        ) -> BlobResult<ListPage> {
            Err(BlobError::Denied)
        }
        fn read_register(&self, k: &str, b: &mut [u8]) -> BlobResult<Option<RegisterRead>> {
            self.inner.read_register(k, b)
        }
        fn swap_register(&self, k: &str, e: u64, v: &[u8]) -> BlobResult<SwapOutcome> {
            self.inner.swap_register(k, e, v)
        }
        fn create_register(&self, k: &str, v: &[u8]) -> BlobResult<SwapOutcome> {
            self.inner.create_register(k, v)
        }
        fn delete_register(&self, k: &str, e: u64) -> BlobResult<bool> {
            self.inner.delete_register(k, e)
        }
    }
}

use faults::Recording;

fn publish_through(
    store: &Arc<Emulator>,
    fail_at: Option<usize>,
    take: &Take<'_>,
) -> (Result<oss_meta::Published, ExportError>, Vec<String>) {
    let (got, log, _) = publish_reading(store, fail_at, None, take);
    (got, log)
}

/// A take that fails its write `fail_at` or its read `fail_read_at`; the
/// writes it made and the reads it made.
fn publish_reading(
    store: &Arc<Emulator>,
    fail_at: Option<usize>,
    fail_read_at: Option<usize>,
    take: &Take<'_>,
) -> (Result<oss_meta::Published, ExportError>, Vec<String>, usize) {
    let rec = Recording {
        inner: ExportStore::new(store.clone(), PREFIX),
        log: std::sync::Mutex::new(Vec::new()),
        fail_at,
        reads: std::sync::Mutex::new(0),
        fail_read_at,
    };
    let target = ExportTarget {
        store: &rec,
        prefix: PREFIX,
    };
    let got = (|| {
        let base = open_export_base(&target)?;
        let id = base
            .index_id(take.index)
            .map_or_else(|| format!("ID{}", take.index), str::to_owned);
        let settings: Vec<(String, String)> = Vec::new();
        let indices = [ExportIndex {
            name: take.index,
            id: &id,
            uuid: take.index_uuid,
            creation_date_ms: 1,
            mapping: None,
            settings: &settings,
            shards: &take.shards,
        }];
        let snap = ExportSnapshot {
            name: take.name,
            uuid: take.uuid,
            start_ms: 1,
            end_ms: 2,
            indices: &indices,
        };
        publish_snapshot(&target, base, &snap, &mut [0u8; 64])
    })();
    let log = rec.log.lock().unwrap().clone();
    let reads = *rec.reads.lock().unwrap();
    (got, log, reads)
}

/// Everything the generation `index.latest` names reads back: each
/// snapshot's info, and each of its indices' metadata, shard snapshots and
/// the shard generations the index names.
fn assert_restorable(store: &Emulator) {
    let repo = StockRepo::new(store, PREFIX);
    let Ok(generation) = repo.generation() else {
        return;
    };
    let data = repo.repository_data(generation).unwrap();
    for snap in &data.snapshots {
        repo.snapshot_info(&snap.uuid).unwrap();
        for index in data.indices_of(snap) {
            let meta = data.metadata_blob_id(snap, index).unwrap();
            repo.index_metadata(&index.id, meta).unwrap();
            for shard in 0..index.shard_generations.len() {
                let shard = u32::try_from(shard).unwrap();
                repo.shard_snapshot(&index.id, shard, &snap.uuid).unwrap();
            }
        }
    }
    for index in &data.indices {
        for (n, generation) in index.shard_generations.iter().enumerate() {
            let key = format!(
                "{PREFIX}indices/{}/{n}/index-{}",
                index.id,
                generation.as_deref().unwrap()
            );
            assert!(store.head(&key).unwrap().is_some(), "{key}");
        }
    }
}

#[test]
fn the_writes_are_ordered_so_index_latest_is_last_and_follows_the_index_blob() {
    let store = store();
    let (got, log) = publish_through(&store, None, &first());
    got.unwrap();
    let last = log.last().unwrap();
    assert_eq!(last, "put repo/index.latest");
    assert_eq!(log[log.len() - 2], "create repo/index-0");
    let at = |needle: &str| log.iter().position(|l| l.contains(needle)).unwrap();
    // Every blob a restore of the snapshot reads comes before index-0.
    for needle in ["snap-U1.dat", "meta-U1.dat", "/meta-", "/index-"] {
        assert!(at(needle) < at("repo/index-0"), "{needle}: {log:?}");
    }
    assert!(log.iter().position(|l| l.contains("snap-U1.dat")).is_some());
}

#[test]
fn a_fault_at_every_write_leaves_a_repository_that_still_restores() {
    let writes = {
        let store = store();
        publish_through(&store, None, &first()).1.len()
    };
    assert!(writes >= 9);
    for k in 0..writes {
        // A repository that already holds a snapshot, then a take that fails at write k.
        let store = store();
        publish(&store, None, &first()).unwrap();
        let second = Take {
            name: "s2",
            uuid: "U2",
            shards: shards("q", 2),
            ..first()
        };
        let (got, _) = publish_through(&store, Some(k), &second);
        assert!(got.is_err(), "write {k} failed but the take did not");
        assert_restorable(&store);
        let repo = StockRepo::new(&*store, PREFIX);
        let data = repo.repository_data(repo.generation().unwrap()).unwrap();
        // The failed snapshot is named only when its generation is the newest and complete.
        assert!(data.snapshots.iter().any(|s| s.name == "s1"));
        // A take after the fault goes through and keeps every earlier snapshot.
        let retry = Take {
            name: "s3",
            uuid: "U3",
            shards: shards("r", 2),
            ..first()
        };
        publish(&store, None, &retry).unwrap();
        assert_restorable(&store);
    }
}

#[test]
fn a_fault_at_every_write_of_a_first_take_leaves_an_empty_or_restorable_repository() {
    let writes = {
        let store = store();
        publish_through(&store, None, &first()).1.len()
    };
    for k in 0..writes {
        let store = store();
        let (got, _) = publish_through(&store, Some(k), &first());
        assert!(got.is_err());
        assert_restorable(&store);
    }
}

#[test]
fn a_fault_at_every_read_of_a_second_take_fails_it_and_leaves_the_repository_restorable() {
    let second = Take {
        name: "s2",
        uuid: "U2",
        shards: shards("q", 2),
        ..first()
    };
    let reads = {
        let store = store();
        publish(&store, None, &first()).unwrap();
        let (got, _, reads) = publish_reading(&store, None, None, &second);
        got.unwrap();
        reads
    };
    // The take reads `index.latest`, `index-N` and the held shard generations.
    assert!(reads >= 3, "{reads} reads");
    for k in 0..reads {
        let store = store();
        publish(&store, None, &first()).unwrap();
        let (got, log, _) = publish_reading(&store, None, Some(k), &second);
        assert!(got.is_err(), "read {k} failed but the take did not");
        assert_restorable(&store);
        let repo = StockRepo::new(&*store, PREFIX);
        let data = repo.repository_data(repo.generation().unwrap()).unwrap();
        assert!(
            data.snapshot("s2").is_none(),
            "read {k}: the failed take is not named: {log:?}"
        );
        assert!(data.snapshot("s1").is_some());
        // The next take goes through and keeps the first.
        let retry = Take {
            name: "s3",
            uuid: "U3",
            shards: shards("r", 2),
            ..first()
        };
        publish(&store, None, &retry).unwrap();
        assert_restorable(&store);
    }
}
