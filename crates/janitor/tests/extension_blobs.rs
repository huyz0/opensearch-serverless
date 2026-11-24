//! docs/specs/extension-abi AC-7: the janitor never touches a stored
//! extension module.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_format::IndexRecord;
use oss_janitor::orphans::{Config, PointInTimeSource, SnapshotSource, Sweeper};
use oss_meta::{DeleteMode, DescriptorStore, PointInTimeStore};

const BASE: &str = "d/";
const GONE: &str = "gone#AAAAAAAAAAAAAAAAAAAAAA#0";
const MODULE: &str =
    "d/ext/modules/0000000000000000000000000000000000000000000000000000000000000000.wasm";

fn put(store: &Emulator, key: &str) {
    let mut buf = [0u8; 64];
    store
        .put(key, &mut SliceSource::new(b"x"), &mut buf)
        .unwrap();
}

#[test]
fn a_sweep_that_deletes_an_orphan_leaves_the_modules_beside_it() {
    let emulator = Arc::new(Emulator::new(64 << 20, 1));
    let store = Arc::clone(&emulator) as Arc<dyn BlobStore>;
    let descriptors =
        DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    descriptors
        .create(
            &IndexRecord::new("live", "BBBBBBBBBBBBBBBBBBBBBB", 1),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .unwrap();
    let clock = Arc::new(AtomicU64::new(1_000));
    let pits = PointInTimeStore::new(
        Arc::clone(&store),
        BASE,
        Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)),
    )
    .unwrap();
    put(&emulator, MODULE);
    put(&emulator, &format!("d/segments/{GONE}/a"));
    let (pit, snap) = (
        PointInTimeSource::new(&pits, 100),
        SnapshotSource::new(Arc::clone(&store), BASE, 100, 1 << 16),
    );
    let config = Config {
        dry_run: false,
        max_pins: 1000,
        descriptor_bytes: 1 << 16,
    };
    let done = Sweeper::new(emulator.as_ref(), &descriptors, BASE, config, &pit, &snap)
        .slice(100)
        .unwrap();
    assert_eq!(done.orphans, 1, "the orphan was swept");
    assert!(
        emulator.head(MODULE).unwrap().is_some(),
        "the module is still there"
    );
}
