//! Removing an index's shard heads (docs/specs/index-delete/SPEC.md, AC-1 to AC-4).

use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::{HeadRegisters, ShardHeadStore, shard_head_key};
use oss_cluster::{LeaseClock, LivenessOracle};

use crate::{BASE, Fleet, TTL};

fn take(fleet: &Fleet, index: &str, shard: u32, uuid: Option<&str>) {
    let store: Arc<dyn BlobStore> = fleet.store.clone();
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    let oracle = Arc::new(LivenessOracle::new(Arc::new(fleet.node())));
    ShardHeadStore::new(store, BASE, clock, TTL, oracle)
        .unwrap()
        .acquire(index, shard, "n1", "en1", uuid)
        .unwrap();
}

fn registers(fleet: &Fleet) -> HeadRegisters {
    let store: Arc<dyn BlobStore> = fleet.store.clone();
    HeadRegisters::new(store, BASE).unwrap()
}

fn exists(fleet: &Fleet, index: &str, shard: u32) -> bool {
    let mut buf = [0u8; 4096];
    fleet
        .store
        .read_register(&shard_head_key(BASE, index, shard), &mut buf)
        .unwrap()
        .is_some()
}

#[test]
fn a_read_finds_the_heads_of_the_uuid_one_get_a_shard() {
    let fleet = Fleet::new();
    take(&fleet, "logs", 0, Some("u1"));
    take(&fleet, "logs", 2, None);
    let before = fleet.total();
    let found = registers(&fleet).read_all_of("logs", 3, "u1").unwrap();
    assert_eq!(fleet.total() - before, 3);
    assert_eq!(fleet.count(S3Op::List), 0);
    let shards: Vec<_> = found.iter().map(|h| (h.shard, h.owned)).collect();
    assert_eq!(shards, [(0, true), (2, true)]);
}

#[test]
fn another_incarnations_head_is_not_read() {
    let fleet = Fleet::new();
    take(&fleet, "logs", 0, Some("other"));
    assert!(
        registers(&fleet)
            .read_all_of("logs", 1, "u1")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_head_is_deleted_only_at_the_generation_read() {
    let fleet = Fleet::new();
    take(&fleet, "logs", 0, Some("u1"));
    take(&fleet, "logs", 1, Some("u1"));
    let regs = registers(&fleet);
    let found = regs.read_all_of("logs", 2, "u1").unwrap();
    // Shard 1 moves after the read.
    let key = shard_head_key(BASE, "logs", 1);
    let mut buf = [0u8; 4096];
    let read = fleet.store.read_register(&key, &mut buf).unwrap().unwrap();
    fleet
        .store
        .swap_register(&key, read.generation, &buf[..read.len])
        .unwrap();
    regs.delete_if_unchanged("logs", &found).unwrap();
    assert!(!exists(&fleet, "logs", 0));
    assert!(exists(&fleet, "logs", 1));
}

#[test]
fn delete_all_of_removes_only_the_uuids_heads_and_counts_them() {
    let fleet = Fleet::new();
    take(&fleet, "logs", 0, Some("u1"));
    take(&fleet, "logs", 1, Some("u2"));
    let regs = registers(&fleet);
    assert_eq!(regs.delete_all_of("logs", 3, "u1").unwrap(), 1);
    assert!(!exists(&fleet, "logs", 0));
    assert!(exists(&fleet, "logs", 1));
    assert_eq!(regs.delete_all_of("logs", 3, "u1").unwrap(), 0);
}

#[test]
fn a_release_clears_the_owner_and_keeps_the_term() {
    let fleet = Fleet::new();
    take(&fleet, "logs", 0, Some("u1"));
    let regs = registers(&fleet);
    regs.release_all_of("logs", 1, "u1").unwrap();
    let found = regs.read_all_of("logs", 1, "u1").unwrap();
    assert_eq!(found.len(), 1);
    assert!(!found[0].owned);
    let mut buf = [0u8; 4096];
    let read = fleet
        .store
        .read_register(&shard_head_key(BASE, "logs", 0), &mut buf)
        .unwrap()
        .unwrap();
    let head = oss_format::ShardHead::parse(&buf[..read.len]).unwrap();
    assert_eq!(head.term, 1);
    assert_eq!(head.lease_expires_at_millis, 0);
}
