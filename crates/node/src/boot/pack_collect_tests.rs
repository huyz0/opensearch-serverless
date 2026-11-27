use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oss_blob::{Emulator, SliceSource};
use oss_cluster::lease::SystemClock;
use oss_format::{PackSidecar, PlannedBlock, SidecarEpoch, encode_node_pack, node_pack_key};
use oss_mem::{MemoryBudget, Pool};
use oss_writer::pack::{PackIndexStore, PackSidecarStore, WINDOW_MILLIS};

use super::*;

/// The thread runs the pass each tick: a pack of an index nothing names is
/// kept at the first window and deleted at the second.
#[test]
fn the_thread_runs_the_pass_each_tick() {
    let emulator = Arc::new(Emulator::new(1 << 24, 3));
    let store: Arc<dyn BlobStore> = Arc::clone(&emulator) as Arc<dyn BlobStore>;
    let key = node_pack_key("", "writer", 1, 0).expect("key");
    let planned = [PlannedBlock {
        index_uuid: "u1",
        index_name: "gone",
        shard: 0,
        name: "_0.seg",
        length: 4,
    }];
    let mut bytes = encode_node_pack("writer", 1, 0, &planned)
        .expect("encode")
        .bytes;
    bytes.extend_from_slice(&[0; 4]);
    emulator
        .put(&key, &mut SliceSource::new(&bytes), &mut [0; 16])
        .expect("put");
    PackIndexStore::new(Arc::clone(&store), "")
        .add("writer", 1)
        .expect("indexed");
    let sidecar = PackSidecarStore::new(Arc::clone(&store), "", "writer").expect("sidecar");
    let state = sidecar.read().expect("read");
    sidecar
        .update(state, |_| PackSidecar::new(vec![SidecarEpoch::new(1, 1)]))
        .expect("update");
    emulator.advance_clock(16 * 60_000);
    let clock = Arc::new(AtomicU64::new(WINDOW_MILLIS));
    let ticking = Arc::clone(&clock);
    let _thread = spawn(
        Parts {
            store: Arc::clone(&store),
            base: String::new(),
            node_id: "me".to_owned(),
        },
        Duration::from_millis(10),
        Box::new(|| vec!["me".to_owned()]),
        Arc::new(move || ticking.load(Ordering::SeqCst)),
        Arc::new(SystemClock),
        Tracking {
            cap: MAX_TRACKED,
            held: None,
        },
    )
    .expect("thread");
    // The first window sights the pack; the second, a window later, deletes it.
    std::thread::sleep(Duration::from_millis(300));
    assert!(emulator.head(&key).expect("head").is_some(), "seen once");
    clock.store(WINDOW_MILLIS * 2, Ordering::SeqCst);
    let started = Instant::now();
    while emulator.head(&key).expect("head").is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "never deleted");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn budget_of(open_indexes: usize) -> MemoryBudget {
    MemoryBudget::new(
        &oss_mem::BudgetConfig::new(open_indexes)
            .with_pool(Pool::OpenIndexes, open_indexes)
            .expect("fits"),
    )
}

/// A pool with room gives the whole tracking cap, and the reservation is
/// what the pool counts as used until it is dropped.
#[test]
fn tracking_is_reserved_from_the_pool_in_full_when_it_fits() {
    let budget = budget_of(MAX_TRACKED * ENTRY_BYTES * 2);
    let (cap, held) = reserve_tracking(&budget);
    assert_eq!(cap, MAX_TRACKED);
    assert_eq!(budget.in_use(Pool::OpenIndexes), MAX_TRACKED * ENTRY_BYTES);
    drop(held);
    assert_eq!(budget.in_use(Pool::OpenIndexes), 0);
}

/// A pool too small for the cap lowers the cap to what it holds.
#[test]
fn a_small_pool_lowers_the_cap_to_what_it_holds() {
    let budget = budget_of(ENTRY_BYTES * 10 + 5);
    let (cap, held) = reserve_tracking(&budget);
    assert_eq!(cap, 10);
    assert_eq!(budget.in_use(Pool::OpenIndexes), 10 * ENTRY_BYTES);
    assert_eq!(held.map(|r| r.bytes()), Some(10 * ENTRY_BYTES));
}

/// A pool with no room tracks nothing, so nothing is judged or deleted.
#[test]
fn a_pool_with_no_room_tracks_nothing() {
    let budget = budget_of(ENTRY_BYTES - 1);
    let (cap, held) = reserve_tracking(&budget);
    assert_eq!(cap, 0);
    assert!(held.is_none());
    assert_eq!(budget.in_use(Pool::OpenIndexes), 0);
}

/// The log line is written only when a pass stopped tracking at the cap.
#[test]
fn the_cap_is_logged_only_when_reached() {
    assert!(cap_line(&Collected::default(), 7).is_none());
    let at_cap = Collected {
        at_cap: true,
        ..Collected::default()
    };
    let line = cap_line(&at_cap, 7).expect("a line");
    assert!(line.contains('7'), "names the cap: {line}");
}
