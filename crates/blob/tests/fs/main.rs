//! The blob-store contract, checked against the filesystem store
//! (docs/specs/fs-store/SPEC.md). Every test asserts exact request counts.

mod layout;
mod listing;
mod writes;

use oss_blob::{BlobResult, BlobStore, ByteRange, FsStore, Meter, S3Op, SliceSource};
use oss_testkit::TempDir;

/// A store on a fresh directory, and the directory, which must outlive it.
pub(crate) fn store(label: &str) -> (FsStore, TempDir) {
    let dir = TempDir::new(label);
    let store = FsStore::new(dir.path().join("store")).expect("open the store");
    (store, dir)
}

/// Asserts the meter holds exactly `expected` attempts and nothing else.
pub(crate) fn assert_counts(meter: &Meter, expected: &[(S3Op, u64)]) {
    for op in S3Op::ALL {
        let want = expected
            .iter()
            .find(|(o, _)| *o == op)
            .map_or(0, |(_, n)| *n);
        assert_eq!(meter.count(op), want, "{op:?}");
    }
}

pub(crate) fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    store
        .put(key, &mut SliceSource::new(bytes), &mut [0u8; 4096])
        .unwrap();
}

pub(crate) fn get_all(store: &dyn BlobStore, key: &str, range: ByteRange) -> BlobResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut sink = |c: &[u8]| {
        out.extend_from_slice(c);
        Ok(())
    };
    store.get(key, range, &mut [0u8; 1000], &mut sink)?;
    Ok(out)
}

pub(crate) fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// Every key the store lists under `prefix`, a page of `page` at a time.
pub(crate) fn list_all(store: &dyn BlobStore, prefix: &str, page: usize) -> Vec<(String, u64)> {
    let mut keys = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut seen = Vec::new();
        let out = store
            // LIST: the operation under test.
            .list_prefix(prefix, after.as_deref(), page, &mut |k, n| {
                seen.push((k.to_owned(), n))
            })
            .unwrap();
        assert_eq!(out.keys, seen.len());
        after = seen.last().map(|(k, _)| k.clone());
        keys.extend(seen);
        if !out.truncated {
            return keys;
        }
    }
}
