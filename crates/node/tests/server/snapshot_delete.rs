//! docs/specs/snapshot-delete: a shallow snapshot deleted over REST, as the
//! shell deletes one, at the request cost the budget table holds.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_testkit::budget;

use crate::snapshot_take::{TAKE, index, pinned, publish, repository, requests, rig, send};

fn taken() -> crate::snapshot_take::Rig {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE)).status,
        200
    );
    r
}

#[test]
fn a_delete_releases_the_name_and_the_pin_and_the_name_can_be_taken_again() {
    let r = taken();
    assert!(pinned(&r, "logs", "u1"));
    let gone = send(&r, "DELETE", "/_snapshot/backups/daily", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert_eq!(gone.text(), r#"{"acknowledged":true}"#);
    assert_eq!(
        send(&r, "GET", "/_snapshot/backups/daily", None).status,
        404
    );
    assert!(!pinned(&r, "logs", "u1"), "a deleted snapshot pins nothing");
    // The repository holds no snapshot marker now, so it can go.
    assert_eq!(send(&r, "DELETE", "/_snapshot/backups", None).status, 200);
    repository(&r, true);
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups/daily", Some(TAKE)).status,
        200
    );
    assert!(pinned(&r, "logs", "u1"));
}

#[test]
fn deleting_what_is_not_there_is_404_and_a_second_delete_too() {
    let r = taken();
    let missing = send(&r, "DELETE", "/_snapshot/backups/nope", None);
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(missing.text().contains("snapshot_missing_exception"));
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/daily", None).status,
        200
    );
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/daily", None).status,
        404
    );
}

#[test]
fn a_selector_or_a_bad_name_is_refused_and_a_switch_off_is_501() {
    let r = taken();
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/_all", None).status,
        501
    );
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/a,b", None).status,
        501
    );
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/_x", None).status,
        400
    );
    let off = rig(false);
    assert_eq!(
        send(&off, "DELETE", "/_snapshot/backups/daily", None).status,
        501
    );
}

#[test]
fn a_snapshot_that_copied_its_data_is_refused_and_left_alone() {
    let r = taken();
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let taker = oss_meta::SnapshotTaker::new(Arc::clone(&blobs), "");
    let draft = oss_format::SnapshotDraft {
        repo: "backups".into(),
        name: "standard".into(),
        uuid: "s".into(),
        storage_generation: Some("genBBBBBBBBBBBBBBBBBBB".into()),
        shallow: false,
        start_millis: 1,
        end_millis: 1,
        global_state: None,
        indices: Vec::new(),
    };
    taker.reserve(&draft, &mut vec![0u8; 1 << 16]).unwrap();
    let got = send(&r, "DELETE", "/_snapshot/backups/standard", None);
    assert_eq!(got.status, 501, "{}", got.text());
    assert_ne!(
        send(&r, "GET", "/_snapshot/backups/standard", None).status,
        404,
        "it is still there"
    );
}

#[test]
fn a_take_still_in_progress_can_be_deleted() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let taker = oss_meta::SnapshotTaker::new(Arc::clone(&blobs), "");
    let described = taker
        .describe(&r.descriptors, "logs", &mut vec![0u8; 1 << 16])
        .unwrap();
    let draft = oss_format::SnapshotDraft {
        repo: "backups".into(),
        name: "stuck".into(),
        uuid: "p".into(),
        storage_generation: Some("genCCCCCCCCCCCCCCCCCCC".into()),
        shallow: true,
        start_millis: 1,
        end_millis: 1,
        global_state: None,
        indices: vec![described.capture(Vec::new(), false)],
    };
    let mut value = vec![0u8; 1 << 16];
    let reservation = taker.reserve(&draft, &mut value).unwrap();
    taker
        .publish_marker(&reservation, &draft, &mut value)
        .unwrap();
    assert!(pinned(&r, "logs", "u1"));
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/stuck", None).status,
        200
    );
    assert!(!pinned(&r, "logs", "u1"));
    // The take that was running finds its capture gone, and cleans up.
    assert!(taker.finish(&reservation, &draft, &mut value).is_err());
    taker.abort(&reservation, &mut value).unwrap();
}

#[test]
fn a_delete_costs_what_the_spec_budgets() {
    let r = taken();
    let before = requests(&r);
    assert_eq!(
        send(&r, "DELETE", "/_snapshot/backups/daily", None).status,
        200
    );
    budget::check(
        "snapshot.delete.requests",
        requests(&r) - before,
        "a delete of one shallow snapshot",
    );
}
