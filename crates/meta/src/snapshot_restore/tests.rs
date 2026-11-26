//! The unmarking after a restore; compiled only under `#[cfg(test)]` from
//! the parent module.

use super::*;
use crate::descriptors::DeleteMode;
use oss_blob::Emulator;

const RECORD: &[u8] =
    br#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
  "indices":{"logs":{"uuid":"U1","number_of_shards":1,"settings":{"index":{"x":1}},"shards":[]}}}"#;

fn marked_index() -> (DescriptorStore, RestoreIndex, u64) {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
    let d = DescriptorStore::new(store, "", DeleteMode::Conditional).unwrap();
    let source = RestoreIndex::find(RECORD, "logs").unwrap().unwrap();
    let marked = mark_restoring(source.settings.as_deref()).unwrap();
    let mut record = restored_record(&source, "logs2", "U2", 1);
    record.settings = &marked;
    let generation = d.create(&record, 1, &mut vec![0u8; 4096]).unwrap();
    (d, source, generation)
}

#[test]
fn a_change_that_moved_the_register_is_kept_and_the_mark_still_goes() {
    let (d, source, generation) = marked_index();
    // Another change lands first: the register's generation is no longer the create's.
    let mut buf = vec![0u8; 4096];
    let mut current = d.get("logs2", &mut buf).unwrap().unwrap();
    current.aliased_by = vec![Cow::Borrowed("everything")];
    d.update(&current, generation, &mut vec![0u8; 4096])
        .unwrap()
        .unwrap();
    SnapshotRestorer::unmark(
        &d,
        &source,
        "logs2",
        "U2",
        1,
        generation,
        &mut vec![0u8; 4096],
    )
    .unwrap();
    let mut buf = vec![0u8; 4096];
    let got = d.get("logs2", &mut buf).unwrap().unwrap();
    assert!(!oss_format::is_restoring(got.settings));
    assert_eq!(got.settings, br#"{"index":{"x":1}}"#);
    assert_eq!(
        got.aliased_by,
        vec![Cow::Borrowed("everything")],
        "the racing change is kept"
    );
}

#[test]
fn an_index_made_again_under_the_name_is_left_alone() {
    let (d, source, generation) = marked_index();
    d.delete("logs2").unwrap();
    let other = IndexRecord::new("logs2", "U9", 3);
    d.create(&other, 2, &mut vec![0u8; 4096]).unwrap();
    let err = SnapshotRestorer::unmark(
        &d,
        &source,
        "logs2",
        "U2",
        1,
        generation,
        &mut vec![0u8; 4096],
    )
    .unwrap_err();
    assert!(matches!(err, RestoreError::Unsupported(_)), "{err}");
    let mut buf = vec![0u8; 4096];
    let got = d.get("logs2", &mut buf).unwrap().unwrap();
    assert_eq!((got.uuid.as_ref(), got.number_of_shards), ("U9", 3));
    assert_eq!(
        got.settings, EMPTY_SETTINGS,
        "the new index's settings are untouched"
    );
}

#[test]
fn another_restores_index_under_the_name_is_left_alone_and_so_is_an_unmarked_one() {
    let (d, source, generation) = marked_index();
    let marked = mark_restoring(None).unwrap();
    // Made again by another restore: marked, but not this restore's uuid.
    d.delete("logs2").unwrap();
    let mut other = IndexRecord::new("logs2", "U9", 1);
    other.settings = &marked;
    d.create(&other, 2, &mut vec![0u8; 4096]).unwrap();
    let err = SnapshotRestorer::unmark(
        &d,
        &source,
        "logs2",
        "U2",
        1,
        generation,
        &mut vec![0u8; 4096],
    )
    .unwrap_err();
    assert!(matches!(err, RestoreError::Unsupported(_)), "{err}");
    let mut buf = vec![0u8; 4096];
    assert!(oss_format::is_restoring(
        d.get("logs2", &mut buf).unwrap().unwrap().settings
    ));
    // This restore's uuid, but already unmarked by someone: nothing to clear.
    d.delete("logs2").unwrap();
    d.create(&IndexRecord::new("logs2", "U2", 1), 3, &mut vec![0u8; 4096])
        .unwrap();
    let err = SnapshotRestorer::unmark(
        &d,
        &source,
        "logs2",
        "U2",
        1,
        generation,
        &mut vec![0u8; 4096],
    )
    .unwrap_err();
    assert!(matches!(err, RestoreError::Unsupported(_)), "{err}");
}

#[test]
fn an_index_deleted_during_its_restore_is_no_success() {
    let (d, source, generation) = marked_index();
    d.delete("logs2").unwrap();
    let err = SnapshotRestorer::unmark(
        &d,
        &source,
        "logs2",
        "U2",
        1,
        generation,
        &mut vec![0u8; 4096],
    )
    .unwrap_err();
    assert!(matches!(err, RestoreError::Unsupported(_)), "{err}");
}
