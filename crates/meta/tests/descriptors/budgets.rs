//! docs/specs/request-budgets/SPEC.md AC-5: the descriptor store, index
//! create and delete, an alias update and a cluster settings update, held
//! to docs/request-budgets.md on the emulator.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, Rounds};
use oss_format::IndexRecord;
use oss_meta::{AliasRecord, ClusterConfig, DeleteMode, DescriptorStore, HeadAt};
use oss_testkit::budget;

use crate::BASE;
use crate::alias_mutations::{NOW, at_now, index, mutations};
use crate::index_lifecycle::{Heads, NO_PINS, NoHeads, lifecycle, queue, shard_blob};

/// Each call is held this long, so calls sent together overlap.
const DELAY: Duration = Duration::from_millis(20);

/// A measured store, and the same store as the crate takes it.
fn measured() -> (Arc<Rounds<Emulator>>, Arc<dyn BlobStore>) {
    let rounds = Arc::new(Rounds::new(Emulator::new(64 << 20, 1)));
    let store: Arc<dyn BlobStore> = rounds.clone();
    (rounds, store)
}

/// Runs `path` as one measurement and checks its request count, and its
/// round trips when `depth_key` names a row.
fn gate<T>(
    rounds: &Rounds<Emulator>,
    requests_key: &str,
    depth_key: Option<&str>,
    path: impl FnOnce() -> T,
) -> T {
    rounds.reset(DELAY);
    let before = rounds.meter().total();
    let out = path();
    let detail = rounds.detail();
    budget::check(requests_key, rounds.meter().total() - before, &detail);
    if let Some(key) = depth_key {
        budget::check(key, rounds.depth() as u64, &detail);
    }
    rounds.reset(Duration::ZERO);
    out
}

#[test]
fn descriptor_operations_cost_what_the_table_says() {
    let (rounds, store) = measured();
    let d = DescriptorStore::new(store, BASE, DeleteMode::Conditional).unwrap();
    let mut buf = vec![0u8; 4096];
    let r = IndexRecord::new("logs", "u", 1);
    let g = gate(&rounds, "descriptor.create.requests", None, || {
        d.create(&r, NOW, &mut buf).unwrap()
    });
    gate(&rounds, "descriptor.read.requests", None, || {
        drop(d.get("logs", &mut buf).unwrap())
    });
    let g = gate(&rounds, "descriptor.update.requests", None, || {
        d.update(&r, g, &mut buf).unwrap().unwrap()
    });
    let deleted = gate(&rounds, "descriptor.delete.requests", None, || {
        d.delete_if_unchanged("logs", g, NOW).unwrap()
    });
    assert!(deleted);
}

#[test]
fn an_index_create_and_delete_cost_what_the_table_says() {
    let (rounds, store) = measured();
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let q = queue(&store);
    let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
    gate(&rounds, "index.create.requests", None, || {
        l.create_index(&IndexRecord::new("logs", "u1", 2), NOW)
            .unwrap()
    });
    // Two shards, one blob each, no writer holding either.
    let heads = Heads {
        read: vec![HeadAt {
            shard: 1,
            generation: 9,
            owned: false,
        }],
        ..Heads::default()
    };
    let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
    shard_blob(&*store, "logs", "u1", 0, "manifest");
    shard_blob(&*store, "logs", "u1", 1, "t=1/_0.cfs");
    let deleted = gate(
        &rounds,
        "index.delete.requests",
        Some("index.delete.round_trips"),
        || l.delete_index("logs", None, NOW).unwrap(),
    );
    assert!(deleted.is_some());
}

#[test]
fn an_alias_update_costs_what_the_table_says() {
    let (rounds, store) = measured();
    let d = DescriptorStore::new(store, BASE, DeleteMode::Tombstone).unwrap();
    for name in ["a", "b", "c"] {
        index(&d, name);
    }
    let mut m = mutations(&d, &at_now);
    let before = AliasRecord::new("al", &["a", "b"]);
    let g = m.create_alias(&before).unwrap();
    // One member out, one in, one kept.
    let after = AliasRecord::new("al", &["b", "c"]);
    let moved = gate(
        &rounds,
        "alias.update.requests",
        Some("alias.update.round_trips"),
        || m.update_alias(&before, &after, g).unwrap(),
    );
    assert!(moved.is_some());
}

#[test]
fn a_cluster_settings_update_costs_what_the_table_says() {
    let (rounds, store) = measured();
    let c = ClusterConfig::new(store, BASE).unwrap();
    let mut buf = [0u8; 1024];
    gate(
        &rounds,
        "cluster_settings.first_update.requests",
        None,
        || c.update(&[("a", Some("1"))], &mut buf).unwrap(),
    );
    gate(
        &rounds,
        "cluster_settings.update.requests",
        Some("cluster_settings.update.round_trips"),
        || c.update(&[("b", Some("2"))], &mut buf).unwrap(),
    );
}
