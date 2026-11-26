//! docs/specs/snapshot-partial AC-9 to AC-11: a restore of a snapshot that
//! is missing shards needs `partial=true` and makes the missing shards empty.

use crate::snapshot_restore::{Rig, restored_uuid, rig_aliased, send};

use oss_blob::{BlobStore, SliceSource};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest};
use oss_node::restore_background::Settings;
use oss_node::stock_restore_route::StockRestoreSettings;

const RESTORE: &str = "/_snapshot/backups/p/_restore?wait_for_completion=true";

/// `logs`, three shards, of which only 0 and 2 published a commit, and the
/// snapshot `p` taken with `partial=true`.
fn partial_snapshot() -> Rig {
    let r = rig_aliased(
        true,
        "",
        64 << 20,
        64,
        StockRestoreSettings::off(),
        Settings::off(),
        false,
    );
    let repo =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some(repo)).status,
        200
    );
    r.descriptors
        .create(
            &IndexRecord::new("logs", "u1", 3),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .unwrap();
    for shard in [0u32, 2] {
        let at = ShardRef {
            index: "logs",
            uuid: "u1",
            shard,
        };
        let key = format!("{}t=1/_0.si", at.base_path("").unwrap());
        r.store
            .put(&key, &mut SliceSource::new(b"payload"), &mut [0u8; 64])
            .unwrap();
        let entries = [ManifestEntry {
            name: "_0.si",
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: 7,
        }];
        let value = encode_manifest(1, Some("n1"), &entries, -1, -1).unwrap();
        r.store
            .create_register(&at.manifest_key("").unwrap(), &value)
            .unwrap();
    }
    let made = send(
        &r,
        "PUT",
        "/_snapshot/backups/p?wait_for_completion=true",
        Some(r#"{"indices":"logs","partial":true}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    r
}

// AC-9: without `partial=true` the restore is refused before any write.
#[test]
fn a_snapshot_missing_shards_is_not_restored_without_partial() {
    let r = partial_snapshot();
    let body = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    let got = send(&r, "POST", RESTORE, Some(body));
    assert_eq!(got.status, 500, "{}", got.text());
    assert!(
        got.text().contains("snapshot_restore_exception") && got.text().contains("partial=true"),
        "{}",
        got.text()
    );
    let mut buf = vec![0u8; 1 << 16];
    assert!(r.descriptors.get("copy", &mut buf).unwrap().is_none());
}

// AC-10: with it the index is whole, the failed shard has no manifest and the answer counts it.
#[test]
fn a_partial_restore_creates_every_shard_and_reports_the_failed_one() {
    let r = partial_snapshot();
    let body = r#"{"rename_pattern":"logs","rename_replacement":"copy","partial":true}"#;
    let got = send(&r, "POST", RESTORE, Some(body));
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text()
            .contains(r#""shards":{"total":3,"failed":1,"successful":2}"#),
        "{}",
        got.text()
    );
    let uuid = restored_uuid(&r, "copy");
    let manifest = |shard: u32| {
        ShardRef {
            index: "copy",
            uuid: &uuid,
            shard,
        }
        .manifest_key("")
        .unwrap()
    };
    let mut buf = vec![0u8; 1 << 16];
    for (shard, present) in [(0, true), (1, false), (2, true)] {
        let read = r.store.read_register(&manifest(shard), &mut buf).unwrap();
        assert_eq!(read.is_some(), present, "shard {shard}");
    }
}

// AC-11: `partial=true` on a snapshot with no failed shard changes nothing.
#[test]
fn partial_true_on_a_whole_snapshot_answers_as_before() {
    let r = crate::snapshot_restore::taken(true);
    let body = r#"{"rename_pattern":"logs","rename_replacement":"copy","partial":true}"#;
    let got = send(&r, "POST", crate::snapshot_restore::RESTORE, Some(body));
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text()
            .contains(r#""shards":{"total":2,"failed":0,"successful":2}"#),
        "{}",
        got.text()
    );
}
