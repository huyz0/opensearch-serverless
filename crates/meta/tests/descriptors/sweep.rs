//! AC-3 to AC-6 of docs/specs/tombstone-sweep/SPEC.md: the tombstone sweep.

use std::cell::Cell;

use oss_blob::BlobStore;
use oss_blob::S3Op::{Delete, Get, GetRange, List, Put};
use oss_format::{IndexRecord, Tombstone};
use oss_meta::{
    BUCKET_MILLIS, DeleteMode, DescriptorStore, MARKERS_PREFIX, REAP_DEADLINE_MILLIS,
    REAP_HANDOFF_MILLIS, Sweep, Swept,
};

use crate::{BASE, Backend, all, counted};

/// 2025-11-15T00:00Z, the start of the hour `2025111500`.
const T0: i64 = 1_763_164_800_000;
const Q: i64 = 6 * BUCKET_MILLIS;

/// Creates `name` and deletes it at `at` with a tombstone and its marker.
fn tombstoned(b: &Backend, name: &'static str, at: i64) {
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut buf = vec![0u8; 4096];
    let g = d
        .create(&IndexRecord::new(name, "u", 1), at, &mut buf)
        .unwrap();
    assert!(d.delete_if_unchanged(name, g, at).unwrap());
}

fn markers(store: &dyn BlobStore) -> Vec<String> {
    let mut keys = Vec::new();
    store
        // LIST: a test reading which markers are left.
        .list_prefix(
            &format!("{BASE}{MARKERS_PREFIX}"),
            None,
            1000,
            &mut |k, _| keys.push(k.to_owned()),
        )
        .unwrap();
    keys
}

fn register(store: &dyn BlobStore, name: &str) -> Option<(u64, Vec<u8>)> {
    let mut buf = [0u8; 4096];
    store
        .read_register(&format!("{BASE}indices/{name}"), &mut buf)
        .unwrap()
        .map(|r| (r.generation, buf[..r.len].to_vec()))
}

/// Runs a sweep at `now`, the clock also reading `now`, with every bucket owned.
fn sweep_at(d: &DescriptorStore, now: i64, page: usize) -> (Swept, Vec<String>) {
    let clock = || now;
    sweep_with(d, now, page, &|_| true, &clock)
}

fn sweep_with(
    d: &DescriptorStore,
    now: i64,
    page: usize,
    owns: &dyn Fn(&str) -> bool,
    clock: &dyn Fn() -> i64,
) -> (Swept, Vec<String>) {
    let mut removed = Vec::new();
    let mut buf = vec![0u8; 4096];
    let swept = d
        .sweep_tombstones(
            &Sweep {
                now_millis: now,
                quarantine_millis: Q,
                page,
                owns_bucket: owns,
                clock,
            },
            &mut buf,
            &mut |name| removed.push(name.to_owned()),
        )
        .unwrap();
    (swept, removed)
}

fn swept(removed: u64, stale: u64, kept: u64) -> Swept {
    Swept {
        removed,
        stale,
        kept,
    }
}

#[test]
fn a_tombstone_past_its_quarantine_is_removed_with_its_marker() {
    assert_eq!(
        oss_meta::DEFAULT_QUARANTINE_MILLIS,
        21_600_000,
        "six hours, as the Java's"
    );
    for b in all() {
        tombstoned(&b, "logs", T0 + 5);
        // A key nested inside the hour is no marker: neither read nor kept.
        b.store
            .put(
                &format!("{BASE}tombstones/2025111500/a/b"),
                &mut oss_blob::SliceSource::new(&[]),
                &mut [0u8; 8],
            )
            .unwrap();
        let d = b.descriptors(DeleteMode::Conditional);
        // The hour is too young: one LIST, no marker read.
        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + Q - 1, 1000));
        assert_eq!(out, (swept(0, 0, 0), vec![]), "{:?}", b.kind);
        assert_eq!(n, [(List, 1)], "{:?} young", b.kind);

        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + Q + 10, 1000));
        assert_eq!(
            out,
            (swept(1, 0, 0), vec!["logs".to_owned()]),
            "{:?}",
            b.kind
        );
        assert_eq!(
            n,
            b.budget(
                &[(Get, 1), (Put, 1), (List, 1), (Delete, 2)],
                &[(Get, 1), (GetRange, 2), (Put, 1), (List, 1), (Delete, 2)]
            ),
            "{:?} removed",
            b.kind
        );
        assert_eq!(register(&*b.store, "logs"), None, "{:?}", b.kind);
        assert_eq!(
            markers(&*b.store),
            [format!("{BASE}tombstones/2025111500/a/b")],
            "{:?}",
            b.kind
        );
        b.store
            .delete(&format!("{BASE}tombstones/2025111500/a/b"))
            .unwrap();
        let mut buf = vec![0u8; 4096];
        d.create(&IndexRecord::new("logs", "u2", 1), T0 + Q + 20, &mut buf)
            .unwrap();
        // Nothing left: one LIST.
        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + Q + 30, 1000));
        assert_eq!(out.0, swept(0, 0, 0));
        assert_eq!(n, [(List, 1)], "{:?} empty", b.kind);
    }
}

#[test]
fn a_store_without_conditional_deletes_keeps_every_tombstone() {
    for b in all() {
        tombstoned(&b, "logs", T0);
        let d = b.descriptors(DeleteMode::Tombstone);
        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + 10 * Q, 1000));
        assert_eq!(out.0, swept(0, 0, 0));
        assert_eq!(n, [], "{:?}", b.kind);
        assert!(register(&*b.store, "logs").is_some());
    }
}

#[test]
fn stale_markers_are_dropped_without_touching_the_register() {
    for b in all() {
        let t = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        // Recreated and live.
        tombstoned(&b, "live", T0);
        t.create(&IndexRecord::new("live", "u2", 1), T0 + 1, &mut buf)
            .unwrap();
        // Gone already.
        tombstoned(&b, "gone", T0);
        b.store.delete(&format!("{BASE}indices/gone")).unwrap();
        // Recreated, and deleted again two hours later.
        tombstoned(&b, "again", T0);
        let g = t
            .create(
                &IndexRecord::new("again", "u2", 1),
                T0 + 2 * BUCKET_MILLIS,
                &mut buf,
            )
            .unwrap();
        assert!(
            t.delete_if_unchanged("again", g, T0 + 2 * BUCKET_MILLIS)
                .unwrap()
        );
        let live = register(&*b.store, "live");

        let d = b.descriptors(DeleteMode::Conditional);
        // Only the first hour is old enough.
        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + Q + 10, 1000));
        assert_eq!(out, (swept(0, 3, 0), vec![]), "{:?}", b.kind);
        assert_eq!(n, [(Get, 3), (List, 1), (Delete, 3)], "{:?}", b.kind);
        assert_eq!(register(&*b.store, "live"), live, "{:?} untouched", b.kind);
        assert_eq!(
            markers(&*b.store),
            [format!("{BASE}tombstones/2025111502/again")]
        );
        // Two hours on, the later delete's own marker removes it.
        let (out, _) = counted(b.meter(), || sweep_at(&d, T0 + 2 * BUCKET_MILLIS + Q, 1000));
        assert_eq!(
            out,
            (swept(1, 0, 0), vec!["again".to_owned()]),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn another_sweeps_claim_is_kept_until_its_handoff() {
    for b in all() {
        tombstoned(&b, "logs", T0);
        let now = T0 + Q + 10;
        let key = format!("{BASE}indices/logs");
        let (g, _) = register(&*b.store, "logs").unwrap();
        let mut claim = [0u8; 128];
        let n = Tombstone {
            deleted_at: T0,
            reaping_at: now - REAP_HANDOFF_MILLIS + 1,
        }
        .write(&mut claim)
        .unwrap();
        b.store.swap_register(&key, g, &claim[..n]).unwrap();

        let d = b.descriptors(DeleteMode::Conditional);
        assert_eq!(sweep_at(&d, now, 1000).0, swept(0, 0, 1), "{:?}", b.kind);
        assert!(register(&*b.store, "logs").is_some());
        assert_eq!(markers(&*b.store).len(), 1, "{:?} marker kept", b.kind);
        // A millisecond later the claim is abandoned and claimed afresh.
        assert_eq!(
            sweep_at(&d, now + 1, 1000).0,
            swept(1, 0, 0),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_claim_past_its_deadline_is_left_claimed() {
    for b in all() {
        tombstoned(&b, "logs", T0);
        let d = b.descriptors(DeleteMode::Conditional);
        let now = T0 + Q + 10;
        // The first reading stamps the claim; the second finds the deadline gone.
        let readings = Cell::new(0);
        let clock = || {
            readings.set(readings.get() + 1);
            if readings.get() == 1 {
                now
            } else {
                now + REAP_DEADLINE_MILLIS
            }
        };
        let (out, _) = sweep_with(&d, now, 1000, &|_| true, &clock);
        assert_eq!(out, swept(0, 0, 1), "{:?}", b.kind);
        let (_, value) = register(&*b.store, "logs").unwrap();
        assert_eq!(
            Tombstone::parse(&value).unwrap(),
            Tombstone {
                deleted_at: T0,
                reaping_at: now
            }
        );
        assert_eq!(markers(&*b.store).len(), 1, "{:?} marker kept", b.kind);
        // Just inside the deadline, it is removed.
        let later = now + REAP_HANDOFF_MILLIS;
        let readings = Cell::new(0);
        let clock = || {
            readings.set(readings.get() + 1);
            if readings.get() == 1 {
                later
            } else {
                later + REAP_DEADLINE_MILLIS - 1
            }
        };
        let (out, _) = sweep_with(&d, later, 1000, &|_| true, &clock);
        assert_eq!(out, swept(1, 0, 0), "{:?}", b.kind);
    }
}

#[test]
fn a_tombstone_stamped_ahead_of_the_sweeps_clock_is_kept() {
    for b in all() {
        // Filed in the first hour by a clock running 59 minutes ahead.
        tombstoned(&b, "logs", T0 + BUCKET_MILLIS - 1);
        let d = b.descriptors(DeleteMode::Conditional);
        assert_eq!(sweep_at(&d, T0 + Q, 1000).0, swept(0, 0, 1), "{:?}", b.kind);
        assert_eq!(
            sweep_at(&d, T0 + BUCKET_MILLIS - 1 + Q, 1000).0,
            swept(1, 0, 0),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn buckets_not_owned_and_names_not_hours_are_skipped_unread() {
    for b in all() {
        tombstoned(&b, "a1", T0);
        tombstoned(&b, "a2", T0);
        tombstoned(&b, "a3", T0);
        tombstoned(&b, "b1", T0 + BUCKET_MILLIS);
        // A directory that is not an hour, sorting first, and a stray key.
        let mut empty = oss_blob::SliceSource::new(&[]);
        b.store
            .put(
                &format!("{BASE}tombstones/0000/a1"),
                &mut empty,
                &mut [0u8; 8],
            )
            .unwrap();
        b.store
            .put(
                &format!("{BASE}tombstones/stray"),
                &mut empty,
                &mut [0u8; 8],
            )
            .unwrap();
        let d = b.descriptors(DeleteMode::Conditional);
        let owns = |bucket: &str| bucket != "2025111500";
        let clock = || T0 + 2 * Q;
        let (out, n) = counted(b.meter(), || sweep_with(&d, T0 + 2 * Q, 2, &owns, &clock));
        assert_eq!(out, (swept(1, 0, 0), vec!["b1".to_owned()]), "{:?}", b.kind);
        // Pages: [0000/a1, 2025111500/a1], past the bucket [2025111501/b1, stray];
        // only b1's register is read.
        assert_eq!(
            n.iter().find(|(op, _)| *op == List),
            Some(&(List, 2)),
            "{:?} {n:?}",
            b.kind
        );
        assert_eq!(
            n.iter().find(|(op, _)| *op == Get),
            Some(&(Get, 1)),
            "{:?} {n:?}",
            b.kind
        );
        assert!(register(&*b.store, "a1").is_some());
    }
}

#[test]
fn markers_are_listed_a_page_at_a_time() {
    for b in all() {
        for name in ["m1", "m2", "m3", "m4", "m5"] {
            tombstoned(&b, name, T0);
        }
        let d = b.descriptors(DeleteMode::Conditional);
        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + Q, 2));
        assert_eq!(out.0, swept(5, 0, 0), "{:?}", b.kind);
        assert_eq!(
            n.iter().find(|(op, _)| *op == List),
            Some(&(List, 3)),
            "{:?} {n:?}",
            b.kind
        );
        assert_eq!(markers(&*b.store), Vec::<String>::new());
    }
}

#[test]
fn the_walk_stops_at_the_first_young_hour() {
    for b in all() {
        tombstoned(&b, "old", T0);
        for (name, at) in [
            ("y1", T0 + BUCKET_MILLIS),
            ("y2", T0 + BUCKET_MILLIS),
            ("y3", T0 + 2 * BUCKET_MILLIS),
        ] {
            tombstoned(&b, name, at);
        }
        let d = b.descriptors(DeleteMode::Conditional);
        // Only the first hour is old enough; a page of one key at a time.
        let (out, n) = counted(b.meter(), || sweep_at(&d, T0 + Q, 1));
        assert_eq!(
            out,
            (swept(1, 0, 0), vec!["old".to_owned()]),
            "{:?}",
            b.kind
        );
        assert_eq!(
            n.iter().find(|(op, _)| *op == List),
            Some(&(List, 2)),
            "{:?} {n:?}",
            b.kind
        );
        assert_eq!(
            n.iter().find(|(op, _)| *op == Get),
            Some(&(Get, 1)),
            "{:?} {n:?}",
            b.kind
        );
        assert_eq!(markers(&*b.store).len(), 3, "{:?}", b.kind);
    }
}
