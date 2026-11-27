use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oss_blob::{Emulator, SliceSource};
use oss_cluster::lease::SystemClock;
use oss_format::{PackSidecar, PlannedBlock, SidecarEpoch, encode_node_pack, node_pack_key};
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
