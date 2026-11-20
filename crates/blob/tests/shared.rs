//! docs/specs/blob-shared-store: one store behind a forwarding handle.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, FsStore, S3Op, Shared, SliceSource, SwapOutcome};

fn rig() -> (Arc<Emulator>, Shared) {
    let store = Arc::new(Emulator::new(1 << 24, 3));
    let shared = Shared::new(Arc::clone(&store) as Arc<dyn BlobStore>);
    (store, shared)
}

fn read(store: &dyn BlobStore, key: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    store
        .get(key, ByteRange::Full, &mut buf, &mut |chunk: &[u8]| {
            out.extend_from_slice(chunk);
            Ok(())
        })
        .expect("reads");
    out
}

#[test]
fn objects_and_registers_are_one_store_through_either_handle() {
    let (store, shared) = rig();
    let mut buf = [0u8; 64];
    shared
        .put("a", &mut SliceSource::new(b"one"), &mut buf)
        .expect("puts");
    assert_eq!(read(&*store, "a"), b"one");
    store
        .put("b", &mut SliceSource::new(b"two"), &mut buf)
        .expect("puts");
    assert_eq!(read(&shared, "b"), b"two");
    assert!(shared.head("a").expect("heads").is_some());
    shared.delete("a").expect("deletes");
    assert!(store.head("a").expect("heads").is_none());

    assert!(matches!(
        shared.create_register("r", b"v1").expect("creates"),
        SwapOutcome::Applied(_)
    ));
    let mut value = [0u8; 64];
    let read = store
        .read_register("r", &mut value)
        .expect("reads")
        .expect("there");
    assert!(matches!(
        shared
            .swap_register("r", read.generation, b"v2")
            .expect("swaps"),
        SwapOutcome::Applied(_)
    ));
    let now = shared
        .read_register("r", &mut value)
        .expect("reads")
        .expect("there");
    assert!(now.generation != read.generation);
    assert!(
        shared
            .delete_register("r", now.generation)
            .expect("deletes")
    );
}

#[test]
fn the_shared_store_makes_the_wrapped_stores_requests() {
    let (store, shared) = rig();
    let mut buf = [0u8; 64];
    for key in ["k/1", "k/2", "k/3"] {
        shared
            .put(key, &mut SliceSource::new(b"x"), &mut buf)
            .expect("puts");
    }
    assert_eq!(shared.meter().count(S3Op::Put), 3);
    assert_eq!(store.meter().count(S3Op::Put), 3, "one meter");
    let mut seen = Vec::new();
    // LIST: the test's one page.
    let page = shared
        .list_prefix("k/", None, 10, &mut |k, _| seen.push(k.to_owned()))
        .expect("lists");
    assert_eq!(seen.len(), 3);
    assert!(!page.truncated);
    assert_eq!(store.meter().count(S3Op::List), 1);
    shared.delete_many(&["k/1", "k/2", "k/3"]).expect("deletes");
    assert_eq!(store.meter().count(S3Op::Delete), 1, "one batch");
}

#[test]
fn the_key_rule_is_the_wrapped_stores() {
    let dir = std::env::temp_dir().join(format!("shared-store-{}", std::process::id()));
    let fs = FsStore::new(&dir).expect("fs store");
    let shared = Shared::new(Arc::new(fs));
    assert!(shared.validate_key("a/b").is_ok());
    // The filesystem's rule refuses '..'; the default (S3's) rule does not.
    assert!(shared.validate_key("a/../b").is_err());
    assert!(oss_blob::store::check_key("a/../b").is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_handle_is_named_in_a_debug_print() {
    let (_, shared) = rig();
    assert_eq!(format!("{shared:?}"), "Shared");
}
