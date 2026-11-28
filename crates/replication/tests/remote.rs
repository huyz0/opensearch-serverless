//! docs/specs/replication/SPEC.md AC-2 to AC-4: reading a leader's store.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, PriceClass, S3Op};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_replication::{RemoteError, RemoteStore};
use oss_testkit::budget;

const BASE: &str = "d/";
const UUID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

struct Leader {
    emulator: Arc<Emulator>,
}

impl Leader {
    fn new() -> Self {
        let emulator = Arc::new(Emulator::new(64 << 20, 1));
        let store = Arc::clone(&emulator) as Arc<dyn BlobStore>;
        let descriptors = DescriptorStore::new(store, BASE, DeleteMode::Conditional).unwrap();
        let mut record = IndexRecord::new("logs", UUID, 3);
        record.mapping = Some("{\"properties\":{\"a\":{\"type\":\"keyword\"}}}".into());
        descriptors
            .create(&record, 1, &mut vec![0u8; 1 << 16])
            .unwrap();
        Self { emulator }
    }

    /// Publishes a commit naming `files`, as the writer's publisher does.
    fn publish(&self, shard: u32, term: u64, files: &[&str]) {
        let whole = FileLocation::Whole { term_dir: "t1" };
        let entries: Vec<ManifestEntry<'_>> = files
            .iter()
            .map(|name| ManifestEntry {
                name,
                location: whole.encode(),
                length: 10,
            })
            .collect();
        let bytes = encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap();
        let key = ShardRef {
            index: "logs",
            uuid: UUID,
            shard,
        }
        .manifest_key(BASE)
        .unwrap();
        let mut buf = vec![0u8; 1 << 16];
        let cur = self.emulator.read_register(&key, &mut buf).unwrap();
        match cur {
            None => {
                self.emulator.create_register(&key, &bytes).unwrap();
            }
            Some(read) => {
                self.emulator
                    .swap_register(&key, read.generation, &bytes)
                    .unwrap();
            }
        }
    }

    fn remote(&self) -> RemoteStore {
        RemoteStore::new(Arc::clone(&self.emulator) as Arc<dyn BlobStore>, BASE).unwrap()
    }
}

#[test]
fn a_shard_manifest_is_the_latest_generation_and_its_files() {
    let leader = Leader::new();
    leader.publish(0, 1, &["_0.cfs", "_0.si"]);
    leader.publish(0, 2, &["_0.cfs", "_0.si", "_1.cfs"]);
    let got = leader.remote().shard_manifest("logs", UUID, 0).unwrap();
    assert_eq!(got.term, 2);
    assert_eq!(got.files, ["_0.cfs", "_0.si", "_1.cfs"]);
    let again = leader.remote().shard_manifest("logs", UUID, 0).unwrap();
    assert_eq!(again.generation, got.generation);
    leader.publish(0, 3, &["_2.cfs"]);
    let newer = leader.remote().shard_manifest("logs", UUID, 0).unwrap();
    assert_ne!(newer.generation, got.generation);
    assert_eq!(newer.files, ["_2.cfs"]);
}

#[test]
fn a_descriptor_gives_the_leaders_uuid_shard_count_and_mapping() {
    let leader = Leader::new();
    let index = leader.remote().index_descriptor("logs").unwrap();
    assert_eq!(index.uuid, UUID);
    assert_eq!(index.number_of_shards, 3);
    assert_eq!(
        index.mapping.as_deref(),
        Some("{\"properties\":{\"a\":{\"type\":\"keyword\"}}}")
    );
}

#[test]
fn a_missing_index_or_shard_is_remote_missing() {
    let leader = Leader::new();
    let remote = leader.remote();
    assert_eq!(
        remote.index_descriptor("nope").unwrap_err(),
        RemoteError::Missing
    );
    assert_eq!(
        remote.shard_manifest("logs", UUID, 2).unwrap_err(),
        RemoteError::Missing
    );
}

#[test]
fn a_manifest_this_reader_cannot_read_is_a_typed_format_error() {
    let leader = Leader::new();
    let key = ShardRef {
        index: "logs",
        uuid: UUID,
        shard: 1,
    }
    .manifest_key(BASE)
    .unwrap();
    leader.emulator.create_register(&key, b"not json").unwrap();
    assert!(matches!(
        leader.remote().shard_manifest("logs", UUID, 1).unwrap_err(),
        RemoteError::Format(_)
    ));
}

#[test]
fn a_manifest_costs_one_get_a_descriptor_one_get_and_nothing_lists() {
    let leader = Leader::new();
    leader.publish(0, 1, &["_0.cfs"]);
    let remote = leader.remote();
    let meter = leader.emulator.meter();
    let before = (meter.total(), meter.count(S3Op::Get));
    let puts = meter.class_total(PriceClass::PutClass);
    remote.shard_manifest("logs", UUID, 0).unwrap();
    let manifest = meter.total() - before.0;
    budget::check(
        "replication.shard_manifest.requests",
        manifest,
        "a manifest",
    );
    assert_eq!(meter.count(S3Op::Get) - before.1, manifest);
    remote.index_descriptor("logs").unwrap();
    let descriptor = meter.total() - before.0 - manifest;
    budget::check(
        "replication.descriptor.requests",
        descriptor,
        "a descriptor",
    );
    assert_eq!(meter.count(S3Op::Get) - before.1, manifest + descriptor);
    // No PUT-class request, which is also every LIST: nothing listed or wrote.
    assert_eq!(meter.class_total(PriceClass::PutClass), puts);
}

#[test]
fn an_idle_remote_makes_no_request() {
    let leader = Leader::new();
    let before = leader.emulator.meter().total();
    let _remote = leader.remote();
    budget::check(
        "replication.idle.requests",
        leader.emulator.meter().total() - before,
        "an idle remote",
    );
}
