//! docs/specs/snapshot-delete: a snapshot released and its marker removed,
//! with a store call failing at each point.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_meta::{SnapshotTaker, TakeError};

use crate::BASE;
use crate::probe::{Quirk, Quirky};
use crate::snapshot_take_faults::{draft, markers, read, take};

fn plain() -> Arc<dyn BlobStore> {
    Arc::new(Quirky::new(Quirk::Plain))
}

fn buf() -> Vec<u8> {
    vec![0u8; 1 << 16]
}

#[test]
fn a_remove_releases_the_name_and_the_marker_and_a_second_finds_nothing() {
    let store: Arc<dyn BlobStore> = plain();
    let t = SnapshotTaker::new(Arc::clone(&store), BASE);
    take(&t, "daily", "genAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(markers(&*store), 1);
    assert!(t.remove("backups", "daily", &mut buf()).unwrap());
    assert_eq!(markers(&*store), 0, "the marker is removed");
    assert_eq!(read(&store), None);
    assert!(!t.remove("backups", "daily", &mut buf()).unwrap());
    assert!(!t.remove("backups", "never", &mut buf()).unwrap());
    // The name is free again.
    take(&t, "daily", "genBBBBBBBBBBBBBBBBBBB").unwrap();
    assert_eq!(read(&store), Some(1));
}

#[test]
fn a_snapshot_that_copied_its_data_is_refused_and_stays() {
    let store: Arc<dyn BlobStore> = plain();
    let t = SnapshotTaker::new(Arc::clone(&store), BASE);
    let mut standard = draft("daily", "genAAAAAAAAAAAAAAAAAAA", false);
    standard.shallow = false;
    t.reserve(&standard, &mut buf()).unwrap();
    assert!(matches!(
        t.remove("backups", "daily", &mut buf()),
        Err(TakeError::NotShallow)
    ));
    assert!(read(&store).is_some(), "it is still there");
}

#[test]
fn a_remove_that_fails_at_any_store_call_loses_nothing_and_a_retry_finishes() {
    // A take makes 7 requests; a remove makes 3 more: the register read,
    // the swap, the marker delete.
    for n in 1..=3 {
        let quirky = Arc::new(Quirky::new(Quirk::FailRequest(7 + n)));
        let store: Arc<dyn BlobStore> = Arc::clone(&quirky) as Arc<dyn BlobStore>;
        let t = SnapshotTaker::new(Arc::clone(&store), BASE);
        take(&t, "daily", "genAAAAAAAAAAAAAAAAAAA").unwrap();
        let first = t.remove("backups", "daily", &mut buf());
        assert!(first.is_err(), "call {n} fails the remove");
        // Before the swap the snapshot stands; after it the name is free.
        let stands = read(&store).is_some();
        assert_eq!(stands, n < 3, "call {n}");
        let retried = t.remove("backups", "daily", &mut buf()).unwrap();
        assert_eq!(retried, stands, "call {n}");
        assert_eq!(read(&store), None);
    }
}
