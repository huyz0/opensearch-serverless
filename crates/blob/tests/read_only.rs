//! docs/specs/stock-snapshot-restore AC-7: the source store answers every
//! write, delete and listing with an error and sends nothing to the store
//! it wraps; reads pass through and are metered.

use std::sync::Arc;

use oss_blob::{
    BlobError, BlobStore, ByteRange, ChunkSource, Emulator, ReadOnlyStore, S3Op, SliceSource,
};

fn seeded() -> (Arc<Emulator>, ReadOnlyStore) {
    let inner = Arc::new(Emulator::new(1 << 20, 1));
    inner
        .put("k", &mut SliceSource::new(b"hello"), &mut [0u8; 8])
        .unwrap();
    let store = ReadOnlyStore::new(inner.clone());
    (inner, store)
}

fn body() -> impl ChunkSource {
    SliceSource::new(b"x")
}

#[test]
fn reads_pass_through_and_are_metered() {
    let (inner, ro) = seeded();
    let mut got = Vec::new();
    ro.get(
        "k",
        ByteRange::From { start: 1, len: 3 },
        &mut [0u8; 8],
        &mut |c: &[u8]| {
            got.extend_from_slice(c);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(got, b"ell");
    assert_eq!(ro.head("k").unwrap().map(|m| m.len), Some(5));
    assert!(ro.head("none").unwrap().is_none());
    assert!(ro.read_register("none", &mut [0u8; 32]).unwrap().is_none());
    assert!(ro.validate_key("a/b").is_ok());
    assert!(ro.validate_key("").is_err());
    assert_eq!(inner.meter().count(S3Op::GetRange), 1);
    assert_eq!(ro.meter().count(S3Op::GetRange), 1, "one meter");
}

#[test]
fn every_write_delete_and_listing_is_refused_and_reaches_nothing() {
    let (inner, ro) = seeded();
    let before = inner.meter().total();
    let denied = |r: Result<(), BlobError>| assert_eq!(r.unwrap_err(), BlobError::Denied);
    denied(ro.put("k", &mut body(), &mut [0u8; 8]));
    denied(
        ro.create_if_absent("n", &mut body(), &mut [0u8; 8])
            .map(|_| ()),
    );
    denied(ro.delete("k"));
    denied(ro.delete_many(&["k", "j"]));
    // LIST: refused by the read-only store, never issued.
    denied(ro.list_prefix("", None, 10, &mut |_, _| {}).map(|_| ()));
    // LIST: refused by the read-only store, never issued.
    denied(ro.list_children("", None, 10, &mut |_| {}).map(|_| ()));
    denied(ro.swap_register("r", 0, b"v").map(|_| ()));
    denied(ro.create_register("r", b"v").map(|_| ()));
    denied(ro.delete_register("r", 1).map(|_| ()));
    assert_eq!(inner.meter().total(), before, "nothing reached the store");
    let mut got = Vec::new();
    inner
        .get("k", ByteRange::Full, &mut [0u8; 8], &mut |c: &[u8]| {
            got.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
    assert_eq!(got, b"hello", "the object is as it was");
}
