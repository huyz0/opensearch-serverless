//! docs/specs/janitor-snapshot-pins: a pass keeps every blob of an index
//! incarnation a snapshot names, and deletes nothing when it cannot read one.

// ARITH: small counts.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use oss_blob::{BlobStore, S3Op, SliceSource};
use oss_janitor::gc::Config;
use oss_janitor::orphans::{IndexPins, SnapshotSource};
use oss_testkit::budget;
use proactive_support::*;

const ORPHAN: &str = "segments/logs#u1#0/t=0/orphan";

const LIVE: Config = Config {
    dry_run: false,
    minimum_unreferenced_millis: 0,
    max_watched: 1000,
    max_pinned: 1000,
    relist_after_millis: 0,
};

/// A written, published shard with a blob of an older term nothing names.
fn published_with_an_orphan() -> (Cluster, Node) {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, None);
    seq(&write(&one.role, "a"));
    one.role.refresh(&info(), 0).expect("published");
    put(&c, ORPHAN, b"old");
    (c, one)
}

fn put(c: &Cluster, key: &str, bytes: &[u8]) {
    c.store
        .put(key, &mut SliceSource::new(bytes), &mut [0u8; 16])
        .expect("put");
}

fn present(c: &Cluster, key: &str) -> bool {
    c.store.head(key).expect("heads").is_some()
}

/// Two passes: the first sees the blob unreferenced, the second may delete it.
fn sweep(c: &Cluster, one: &Node) -> (usize, Option<String>) {
    let pass = janitor(c, &one.role);
    let first = pass.run(LIVE);
    let second = pass.run(LIVE);
    (
        first.deleted + second.deleted,
        first.skipped.or(second.skipped),
    )
}

#[test]
fn a_snapshot_of_the_index_keeps_every_blob_of_its_shards() {
    let (c, one) = published_with_an_orphan();
    put(
        &c,
        "snapshots/s1",
        br#"{"name":"s1","indices":{"logs":{"uuid":"u1"}}}"#,
    );
    let (deleted, skipped) = sweep(&c, &one);
    assert_eq!((deleted, skipped), (0, None));
    assert!(present(&c, ORPHAN), "held by the snapshot");
}

#[test]
fn a_snapshot_of_another_index_or_incarnation_holds_nothing_of_this_one() {
    let (c, one) = published_with_an_orphan();
    put(
        &c,
        "snapshots/s1",
        br#"{"indices":{"logs":{"uuid":"other"},"metrics":{"uuid":"u1"}}}"#,
    );
    let (deleted, _) = sweep(&c, &one);
    assert_eq!(deleted, 1);
    assert!(!present(&c, ORPHAN));
}

#[test]
fn a_shard_with_no_snapshot_at_all_is_swept() {
    let (c, one) = published_with_an_orphan();
    let (deleted, _) = sweep(&c, &one);
    assert_eq!(deleted, 1);
    assert!(!present(&c, ORPHAN));
}

#[test]
fn a_snapshot_record_that_cannot_be_read_stops_the_pass_and_deletes_nothing() {
    let (c, one) = published_with_an_orphan();
    put(&c, "snapshots/broken", b"not json");
    let (deleted, skipped) = sweep(&c, &one);
    assert_eq!(deleted, 0);
    assert!(skipped.is_some(), "the pass says it swept nothing");
    assert!(present(&c, ORPHAN));
}

#[test]
fn reading_the_snapshots_costs_what_the_table_says() {
    let c = cluster();
    let source = SnapshotSource::new(c.store.clone(), "", 100, 1 << 16);
    let mut held = IndexPins::new(10);
    let before: u64 = S3Op::ALL.iter().map(|op| c.store.meter().count(*op)).sum();
    source
        .collect_with(&mut held, &mut vec![0u8; 1 << 16])
        .expect("reads");
    let after: u64 = S3Op::ALL.iter().map(|op| c.store.meter().count(*op)).sum();
    budget::check(
        "janitor.pass.snapshot_pins.requests",
        after - before,
        "reading no snapshot",
    );
}

fn total(c: &Cluster) -> u64 {
    S3Op::ALL.iter().map(|op| c.store.meter().count(*op)).sum()
}

#[test]
fn a_shard_a_snapshot_holds_is_not_looked_at_again_until_the_recheck() {
    let (c, one) = published_with_an_orphan();
    put(&c, "snapshots/s1", br#"{"indices":{"logs":{"uuid":"u1"}}}"#);
    let hourly = Config {
        relist_after_millis: 3_600_000,
        ..LIVE
    };
    let pass = janitor(&c, &one.role);
    let first = pass.run(hourly);
    assert_eq!(first.pinned, 1, "{first:?}");
    let before = total(&c);
    for _ in 0..3 {
        let again = pass.run(hourly);
        assert_eq!(again.swept, 0, "{again:?}");
    }
    budget::check(
        "janitor.pass.snapshot_held.requests",
        total(&c) - before,
        "three passes over a shard a snapshot holds",
    );
    // Past the recheck, and the snapshot gone, the blob is swept.
    c.store.delete("snapshots/s1").expect("deleted");
    pass.run(LIVE);
    pass.run(LIVE);
    assert!(!present(&c, ORPHAN));
}

#[test]
fn more_snapshots_than_the_pass_may_hold_stop_it_and_delete_nothing() {
    let (c, one) = published_with_an_orphan();
    for n in 0..101 {
        put(
            &c,
            &format!("snapshots/s{n:03}"),
            br#"{"indices":{"other":{"uuid":"x"}}}"#,
        );
    }
    let (deleted, skipped) = sweep(&c, &one);
    assert_eq!(deleted, 0);
    assert!(skipped.is_some(), "the cap stops the pass");
    assert!(present(&c, ORPHAN));
}

#[test]
fn a_shard_a_point_in_time_pins_is_tried_again_not_held() {
    use std::sync::Arc;

    use oss_format::PointInTime;
    use oss_meta::PointInTimeStore;

    let (c, one) = published_with_an_orphan();
    let pits = PointInTimeStore::new(
        c.store.clone() as Arc<dyn BlobStore>,
        "",
        Arc::new(|| 1_000),
    )
    .expect("pits");
    let id = "AAAAAAAAAAAAAAAAAAAAAA";
    let buf = &mut vec![0u8; 1 << 16];
    pits.create(
        &PointInTime {
            id: id.to_owned(),
            index: "logs".to_owned(),
            index_uuid: Some("u1".to_owned()),
            expires_at_millis: 9_000_000,
            capturing: true,
            shards: Vec::new(),
        },
        buf,
    )
    .expect("view");
    let hourly = Config {
        relist_after_millis: 3_600_000,
        ..LIVE
    };
    let pass = janitor(&c, &one.role);
    let first = pass.run(hourly);
    assert_eq!(first.pinned, 1, "{first:?}");
    // Only a snapshot's hold is remembered: once the view is released the
    // shard is swept at once, not after the recheck.
    assert!(pits.release(id, buf).expect("released"));
    let again = pass.run(hourly);
    assert_eq!(again.swept, 1, "{again:?}");
}
