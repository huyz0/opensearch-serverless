//! docs/specs/snapshot-read AC-4 to AC-9 and AC-11: the store reads, per backend.

use std::path::PathBuf;

use oss_blob::S3Op;
use oss_blob::SliceSource;
use oss_meta::{MAX_REPOSITORIES, SnapshotReadError, SnapshotReader};

use crate::{BASE, Backend, all, counted};

#[allow(clippy::disallowed_methods)] // a fixture a few hundred bytes long
fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn put(b: &Backend, key: &str, body: &[u8]) {
    b.store
        .put(
            &format!("{BASE}{key}"),
            &mut SliceSource::new(body),
            &mut [0u8; 256],
        )
        .unwrap();
}

fn register(b: &Backend, key: &str, value: &[u8]) {
    b.store
        .create_register(&format!("{BASE}{key}"), value)
        .unwrap();
}

fn reader(b: &Backend) -> SnapshotReader {
    SnapshotReader::new(std::sync::Arc::clone(&b.store), BASE)
}

/// `snapshot-states/<sha256 hex of "backups#daily-1">`.
fn state_key(repo: &str, name: &str) -> String {
    oss_format::snapshot_state_key("", repo, name).unwrap()
}

const RELEASED: &[u8] = br#"{"_released":true}"#;

fn requests(b: &Backend, work: impl FnOnce()) -> u64 {
    let (_, n) = counted(b.meter(), work);
    n.iter().map(|(_, c)| c).sum()
}

#[test]
fn a_repository_is_one_read_and_an_absent_one_is_none() {
    for b in all() {
        put(&b, "repositories/backups", &fixture("repository.json"));
        let r = reader(&b);
        let mut buf = vec![0u8; 4096];
        let mut got = None;
        let n = requests(&b, || got = r.repository("backups", &mut buf).unwrap());
        assert_eq!(got.expect("found").name, "backups", "{:?}", b.kind);
        assert_eq!(n, 1, "{:?} a repository", b.kind);
        assert!(r.repository("nope", &mut buf).unwrap().is_none());
    }
}

#[test]
fn a_snapshot_the_java_wrote_is_read_from_its_register_in_one_request() {
    for b in all() {
        register(
            &b,
            &state_key("backups", "daily-1"),
            &fixture("state-finished.bin"),
        );
        let r = reader(&b);
        let mut buf = vec![0u8; 8192];
        let mut got = None;
        let n = requests(&b, || {
            got = r.snapshot("backups", "daily-1", &mut buf).unwrap()
        });
        let found = got.expect("found").summary;
        assert_eq!((found.name.as_str(), found.indices.len()), ("daily-1", 1));
        assert_eq!(n, 1, "{:?} register", b.kind);
    }
}

#[test]
fn a_legacy_snapshot_is_read_after_its_absent_register_in_two_requests() {
    for b in all() {
        put(
            &b,
            "snapshots/backups#daily-1",
            &fixture("snapshot-finished.json"),
        );
        let r = reader(&b);
        let mut buf = vec![0u8; 8192];
        let mut got = None;
        let n = requests(&b, || {
            got = r.snapshot("backups", "daily-1", &mut buf).unwrap()
        });
        assert!(got.is_some(), "{:?}", b.kind);
        assert_eq!(n, 2, "{:?} legacy", b.kind);
    }
}

#[test]
fn a_missing_snapshot_costs_two_requests_and_a_released_one_costs_one() {
    for b in all() {
        let r = reader(&b);
        let mut buf = vec![0u8; 8192];
        let n = requests(&b, || {
            assert!(r.snapshot("backups", "x", &mut buf).unwrap().is_none())
        });
        assert_eq!(n, 2, "{:?} missing", b.kind);
        register(&b, &state_key("backups", "gone"), RELEASED);
        let n = requests(&b, || {
            assert!(r.snapshot("backups", "gone", &mut buf).unwrap().is_none())
        });
        assert_eq!(n, 1, "{:?} released", b.kind);
    }
}

#[test]
fn a_released_register_hides_a_legacy_record_that_has_not_been_removed_yet() {
    for b in all() {
        register(&b, &state_key("backups", "daily-1"), RELEASED);
        put(
            &b,
            "snapshots/backups#daily-1",
            &fixture("snapshot-finished.json"),
        );
        let mut buf = vec![0u8; 8192];
        assert!(
            reader(&b)
                .snapshot("backups", "daily-1", &mut buf)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn a_record_that_is_not_the_snapshot_asked_for_is_refused() {
    for b in all() {
        put(
            &b,
            "snapshots/backups#other",
            &fixture("snapshot-finished.json"),
        );
        let mut buf = vec![0u8; 8192];
        let err = reader(&b)
            .snapshot("backups", "other", &mut buf)
            .unwrap_err();
        assert!(matches!(err, SnapshotReadError::KeyMismatch), "{err}");
    }
}

#[test]
fn a_value_larger_than_the_buffer_is_too_large_not_partial() {
    for b in all() {
        register(
            &b,
            &state_key("backups", "daily-1"),
            &fixture("state-finished.bin"),
        );
        put(
            &b,
            "snapshots/backups#legacy",
            &fixture("snapshot-finished.json"),
        );
        put(&b, "repositories/big", &fixture("repository.json"));
        let r = reader(&b);
        let mut small = vec![0u8; 64];
        for err in [
            r.snapshot("backups", "daily-1", &mut small).unwrap_err(),
            r.snapshot("backups", "legacy", &mut small).unwrap_err(),
            r.repository("big", &mut small).unwrap_err(),
        ] {
            assert!(
                matches!(err, SnapshotReadError::TooLarge),
                "{:?} {err}",
                b.kind
            );
        }
    }
}

#[test]
fn the_listing_omits_an_unreadable_record_and_costs_a_list_and_a_get_each() {
    for b in all() {
        put(&b, "repositories/a", &fixture("repository.json"));
        put(&b, "repositories/b", &fixture("repository.json"));
        put(&b, "repositories/broken", b"not json");
        let r = reader(&b);
        let mut buf = vec![0u8; 4096];
        let mut names = Vec::new();
        let (_, counts) = counted(b.meter(), || {
            names = r.repositories(&mut buf).unwrap();
        });
        assert_eq!(names.len(), 2, "{:?}", b.kind);
        let get = counts
            .iter()
            .find(|(op, _)| *op == S3Op::Get)
            .map_or(0, |c| c.1);
        let list = counts
            .iter()
            .find(|(op, _)| *op == S3Op::List)
            .map_or(0, |c| c.1);
        assert_eq!((get, list), (3, 1), "{:?} {counts:?}", b.kind);
    }
}

#[test]
fn more_repositories_than_the_cap_are_refused() {
    let b = &all()[0];
    for i in 0..=MAX_REPOSITORIES {
        put(
            b,
            &format!("repositories/r{i:04}"),
            &fixture("repository.json"),
        );
    }
    let mut buf = vec![0u8; 4096];
    let err = reader(b).repositories(&mut buf).unwrap_err();
    assert!(
        matches!(err, SnapshotReadError::TooManyRepositories),
        "{err}"
    );
}
