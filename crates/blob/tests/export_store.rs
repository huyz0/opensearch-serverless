//! docs/specs/stock-snapshot-export AC-8: the target store writes only under
//! the repository's prefix, creates every blob but `index.latest` without
//! overwriting, and answers every delete, listing and register call with an
//! error that reaches nothing.

use std::sync::Arc;

use oss_blob::{
    BlobError, BlobStore, ByteRange, ChunkSource, CreateOutcome, Emulator, ExportStore, S3Op,
    SliceSource,
};

fn store(prefix: &str) -> (Arc<Emulator>, ExportStore) {
    let inner = Arc::new(Emulator::new(1 << 20, 1));
    inner
        .put("repo/seed", &mut SliceSource::new(b"hello"), &mut [0u8; 8])
        .unwrap();
    inner
        .put("other/seed", &mut SliceSource::new(b"x"), &mut [0u8; 8])
        .unwrap();
    let export = ExportStore::new(inner.clone(), prefix);
    (inner, export)
}

fn body() -> impl ChunkSource {
    SliceSource::new(b"x")
}

fn read(s: &dyn BlobStore, key: &str) -> Result<Vec<u8>, BlobError> {
    let mut got = Vec::new();
    s.get(key, ByteRange::Full, &mut [0u8; 8], &mut |c: &[u8]| {
        got.extend_from_slice(c);
        Ok(())
    })?;
    Ok(got)
}

#[test]
fn reads_and_heads_pass_through_under_the_prefix_only() {
    let (_, ex) = store("repo/");
    assert_eq!(read(&ex, "repo/seed").unwrap(), b"hello");
    assert_eq!(ex.head("repo/seed").unwrap().map(|m| m.len), Some(5));
    assert!(ex.head("repo/none").unwrap().is_none());
    assert_eq!(read(&ex, "other/seed").unwrap_err(), BlobError::Denied);
    assert_eq!(ex.head("other/seed").unwrap_err(), BlobError::Denied);
}

#[test]
fn a_blob_is_created_once_and_never_overwritten() {
    let (inner, ex) = store("repo/");
    let out = ex
        .create_if_absent("repo/indices/i/0/__a", &mut body(), &mut [0u8; 8])
        .unwrap();
    assert_eq!(out, CreateOutcome::Created);
    let again = ex
        .create_if_absent(
            "repo/indices/i/0/__a",
            &mut SliceSource::new(b"y"),
            &mut [0u8; 8],
        )
        .unwrap();
    assert_ne!(again, CreateOutcome::Created);
    assert_eq!(read(&*inner, "repo/indices/i/0/__a").unwrap(), b"x");
    assert_eq!(
        ex.create_if_absent("other/x", &mut body(), &mut [0u8; 8])
            .unwrap_err(),
        BlobError::Denied
    );
}

#[test]
fn only_index_latest_may_be_put_over() {
    let (inner, ex) = store("repo/");
    ex.put(
        "repo/index.latest",
        &mut SliceSource::new(b"1"),
        &mut [0u8; 8],
    )
    .unwrap();
    ex.put(
        "repo/index.latest",
        &mut SliceSource::new(b"2"),
        &mut [0u8; 8],
    )
    .unwrap();
    assert_eq!(read(&*inner, "repo/index.latest").unwrap(), b"2");
    for key in [
        "repo/seed",
        "repo/index-3",
        "repo/sub/index.latest",
        "other/index.latest",
    ] {
        assert_eq!(
            ex.put(key, &mut body(), &mut [0u8; 8]).unwrap_err(),
            BlobError::Denied,
            "{key}"
        );
    }
    assert_eq!(read(&*inner, "repo/seed").unwrap(), b"hello");
}

#[test]
fn an_empty_prefix_is_the_whole_target_and_keys_may_not_climb() {
    let (_, ex) = store("");
    assert_eq!(read(&ex, "repo/seed").unwrap(), b"hello");
    for key in ["../x", "a/../b", "/abs", "a//b", "a/./b"] {
        assert_eq!(
            ex.create_if_absent(key, &mut body(), &mut [0u8; 8])
                .unwrap_err(),
            BlobError::Denied,
            "{key}"
        );
    }
}

#[test]
fn every_delete_listing_and_register_call_is_refused_and_reaches_nothing() {
    let (inner, ex) = store("repo/");
    let before = inner.meter().total();
    let denied = |r: Result<(), BlobError>| assert_eq!(r.unwrap_err(), BlobError::Denied);
    denied(ex.delete("repo/seed"));
    denied(ex.delete("other/seed"));
    denied(ex.delete_many(&["repo/seed"]));
    // LIST: refused by the export store, never issued.
    denied(
        ex.list_prefix("repo/", None, 10, &mut |_, _| {})
            .map(|_| ()),
    );
    // LIST: refused by the export store, never issued.
    denied(ex.list_children("repo/", None, 10, &mut |_| {}).map(|_| ()));
    denied(ex.swap_register("repo/r", 0, b"v").map(|_| ()));
    denied(ex.create_register("repo/r", b"v").map(|_| ()));
    denied(ex.delete_register("repo/r", 1).map(|_| ()));
    denied(ex.read_register("repo/r", &mut [0u8; 32]).map(|_| ()));
    assert_eq!(inner.meter().total(), before, "nothing reached the store");
    assert_eq!(read(&*inner, "repo/seed").unwrap(), b"hello");
}

#[test]
fn the_meter_is_the_wrapped_stores() {
    let (inner, ex) = store("repo/");
    ex.head("repo/seed").unwrap();
    assert_eq!(
        ex.meter().count(S3Op::Head),
        inner.meter().count(S3Op::Head)
    );
    assert!(ex.meter().count(S3Op::Head) >= 1);
    assert!(ex.validate_key("a/b").is_ok());
    assert!(ex.validate_key("").is_err());
}
