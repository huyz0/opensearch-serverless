//! The pack index registers over the emulator: an add always writes, a
//! removal is a compare-and-swap on the generation read before, and both
//! stop after 16 attempts.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::Hooked;
use oss_blob::register::SwapOutcome;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{pack_index_key, pack_index_slot};
use oss_writer::pack::{PackError, PackIndexStore, Removal};

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

/// `count` node ids that all hash to `slot`.
fn nodes_in_slot(slot: u8, count: usize) -> Vec<String> {
    (0u32..)
        .map(|n| format!("n{n}"))
        .filter(|id| pack_index_slot(id) == slot)
        .take(count)
        .collect()
}

/// AC-4: the first add creates the register naming the node.
#[test]
fn an_add_creates_the_register_and_names_the_node() {
    let e = emulator();
    let index = PackIndexStore::new(dynstore(&e), "");
    index.add("node-1", 1_000).expect("adds");
    let state = index.read(pack_index_slot("node-1")).expect("reads");
    assert!(state.index.contains("node-1"));
    assert_eq!(state.index.stamp("node-1"), Some(1_000));
    assert!(e.head(&pack_index_key("", 3).expect("key")).is_ok());
}

/// AC-4: an add of a node already listed still writes, so the generation
/// moves and a removal made against an older read fails.
#[test]
fn an_add_of_a_listed_node_still_moves_the_generation() {
    let e = emulator();
    let index = PackIndexStore::new(dynstore(&e), "");
    index.add("node-1", 1_000).expect("adds");
    let slot = pack_index_slot("node-1");
    let before = index.read(slot).expect("reads");
    let puts = e.meter().count(S3Op::Put);
    index.add("node-1", 1_000).expect("adds again");
    assert_eq!(e.meter().count(S3Op::Put), puts + 1, "it wrote");
    let after = index.read(slot).expect("reads");
    assert_ne!(after.generation, before.generation);
    assert_eq!(after.index.stamp("node-1"), Some(1_001));
}

/// AC-4: a removal against the generation read before a writer's add loses
/// and leaves the node listed; against a fresh read it removes; for a node
/// not listed it writes nothing.
#[test]
fn a_removal_loses_to_an_add_made_after_its_read() {
    let e = emulator();
    let index = PackIndexStore::new(dynstore(&e), "");
    index.add("node-1", 1).expect("adds");
    let slot = pack_index_slot("node-1");
    let stale = index.read(slot).expect("reads");
    index.add("node-1", 2).expect("the writer's add");
    assert_eq!(
        index.remove(slot, &stale, "node-1").expect("tries"),
        Removal::Lost
    );
    assert!(index.read(slot).expect("reads").index.contains("node-1"));
    let fresh = index.read(slot).expect("reads");
    assert_eq!(
        index.remove(slot, &fresh, "node-1").expect("removes"),
        Removal::Removed
    );
    assert!(!index.read(slot).expect("reads").index.contains("node-1"));
    let puts = e.meter().count(S3Op::Put);
    let after = index.read(slot).expect("reads");
    assert_eq!(
        index.remove(slot, &after, "node-1").expect("nothing"),
        Removal::Absent
    );
    assert_eq!(e.meter().count(S3Op::Put), puts, "nothing written");
}

/// AC-4: an add stops after 16 attempts when the register keeps moving.
#[test]
fn an_add_gives_up_after_sixteen_lost_swaps() {
    let e = emulator();
    let slot = pack_index_slot("node-1");
    PackIndexStore::new(dynstore(&e), "")
        .add("node-1", 1)
        .expect("seeded");
    let swaps = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&swaps);
    let store = Hooked::swapping(&e, move |_, _, _, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(SwapOutcome::Conflict(None))
    });
    let index = PackIndexStore::new(store, "");
    let err = index.add("node-1", 2).expect_err("busy");
    assert!(matches!(err, PackError::IndexBusy), "{err}");
    assert_eq!(swaps.load(Ordering::SeqCst), 16);
    let _ = slot;
}

/// AC-4: many writers adding to one register at once all land.
#[test]
fn contending_adds_all_land_within_the_retry_bound() {
    let e = emulator();
    let nodes = nodes_in_slot(5, 24);
    let index = Arc::new(PackIndexStore::new(dynstore(&e), ""));
    let handles: Vec<_> = nodes
        .chunks(3)
        .map(|chunk| {
            let index = Arc::clone(&index);
            let chunk = chunk.to_vec();
            std::thread::spawn(move || {
                for node in chunk {
                    index.add(&node, 1).expect("lands");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("no panic");
    }
    let listed = index.read(5).expect("reads").index;
    for node in &nodes {
        assert!(listed.contains(node), "{node} was lost");
    }
}
