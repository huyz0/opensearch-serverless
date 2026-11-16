//! docs/specs/index-lifecycle/SPEC.md: the reclaim queue, and finishing a
//! delete once its intent falls due.

use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::Arc;

use oss_blob::S3Op::{Delete, Get, List, Put};
use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_format::{IndexRecord, ReclaimIntent, reclaim_bucket};
use oss_meta::{
    DEFAULT_RECLAIM_DELAY_MILLIS, DeleteMode, DescriptorStore, MetaError, Purged, ReclaimQueue,
};

use crate::alias_mutations::NOW;
use crate::index_lifecycle::{Heads, NO_PINS, NoHeads, keys, lifecycle, queue, shard_blob};
use crate::probe::{Quirk, Quirky};
use crate::{BASE, all, counted};

fn intent(name: &'static str, uuid: &'static str, had_writer: bool) -> ReclaimIntent<'static> {
    ReclaimIntent {
        name: Cow::Borrowed(name),
        uuid: Cow::Borrowed(uuid),
        shards: 1,
        had_writer,
    }
}

fn bucket(millis: i64) -> String {
    String::from_utf8(reclaim_bucket(millis).unwrap().to_vec()).unwrap()
}

/// Drains everything due at `now`, recording the names reclaimed.
fn drain_all(q: &ReclaimQueue, now: i64) -> (usize, Vec<String>) {
    let seen = RefCell::new(Vec::new());
    let taken = q
        .drain(now, &|_| true, &mut |i| {
            seen.borrow_mut().push(i.name.to_string());
            Ok(())
        })
        .unwrap();
    (taken, seen.into_inner())
}

fn put(store: &dyn BlobStore, key: &str, body: &[u8]) {
    store
        .put(
            &format!("{BASE}{key}"),
            &mut SliceSource::new(body),
            &mut [0u8; 64],
        )
        .unwrap();
}

#[test]
fn an_intent_is_one_put_filed_under_the_minute_it_falls_due() {
    for b in all() {
        let q = queue(&b.store);
        let (done, n) = counted(b.meter(), || q.enqueue(&intent("logs", "u1", true), NOW));
        done.unwrap();
        assert_eq!(n, vec![(Put, 1)], "{:?}", b.kind);
        let key = format!(
            "reclaim/{}/logs#u1",
            bucket(NOW + DEFAULT_RECLAIM_DELAY_MILLIS)
        );
        assert_eq!(keys(&*b.store, "reclaim/"), std::slice::from_ref(&key));
        let mut body = Vec::new();
        b.store
            .get(
                &format!("{BASE}{key}"),
                oss_blob::ByteRange::Full,
                &mut [0u8; 64],
                &mut |c: &[u8]| {
                    body.extend_from_slice(c);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            body,
            br#"{"name":"logs","uuid":"u1","shards":1,"had_writer":true}"#
        );
    }
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
    for (delay, uuid) in [(0, "u0"), (-5, "u5")] {
        let q = queue(&store).with_delay_millis(delay);
        q.enqueue(&intent("now", uuid, false), NOW).unwrap();
    }
    assert_eq!(
        keys(&*store, "reclaim/"),
        [
            format!("reclaim/{}/now#u0", bucket(NOW)),
            format!("reclaim/{}/now#u5", bucket(NOW))
        ]
    );
    assert!(matches!(
        queue(&store).enqueue(&intent("x", "a#b", true), NOW),
        Err(MetaError::Format(_))
    ));
    assert!(ReclaimQueue::new(Arc::clone(&store), "d").is_err());
}

#[test]
fn an_intent_is_due_only_once_its_whole_minute_has_passed() {
    for b in all() {
        let q = queue(&b.store).with_delay_millis(0);
        q.enqueue(&intent("logs", "u1", true), NOW).unwrap();
        let start = NOW / 60_000 * 60_000;
        let (nothing, n) = counted(b.meter(), || drain_all(&q, start + 59_999));
        assert_eq!(nothing, (0, vec![]));
        assert_eq!(n, vec![(List, 1)], "{:?}", b.kind);
        let (taken, n) = counted(b.meter(), || drain_all(&q, start + 60_000));
        assert_eq!(taken, (1, vec!["logs".to_owned()]));
        assert_eq!(n, vec![(Get, 1), (List, 1), (Delete, 1)], "{:?}", b.kind);
        assert!(keys(&*b.store, "reclaim/").is_empty());
        let (empty, n) = counted(b.meter(), || drain_all(&q, start + 600_000));
        assert_eq!(empty, (0, vec![]));
        assert_eq!(n, vec![(List, 1)]);
    }
}

#[test]
fn a_drain_stops_at_the_first_bucket_not_yet_due() {
    for b in all() {
        let q = queue(&b.store).with_delay_millis(0);
        q.enqueue(&intent("a", "u", false), NOW).unwrap();
        q.enqueue(&intent("b", "u", false), NOW).unwrap();
        q.enqueue(&intent("later", "u", false), NOW + 120_000)
            .unwrap();
        let (taken, n) = counted(b.meter(), || drain_all(&q, NOW + 60_000));
        assert_eq!(taken, (2, vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(n, vec![(Get, 2), (List, 1), (Delete, 2)], "{:?}", b.kind);
        assert_eq!(keys(&*b.store, "reclaim/").len(), 1);
    }
}

#[test]
fn a_failed_reclaim_keeps_its_intent_and_a_corrupt_one_is_dropped() {
    for b in all() {
        let q = queue(&b.store).with_delay_millis(0);
        q.enqueue(&intent("fails", "u", false), NOW).unwrap();
        let due = bucket(NOW);
        put(&*b.store, &format!("reclaim/{due}/junk#u"), b"not json");
        let long = format!(
            r#"{{"name":"n","uuid":"u","shards":1,"pad":"{}"}}"#,
            "x".repeat(5000)
        );
        put(&*b.store, &format!("reclaim/{due}/long#u"), long.as_bytes());
        put(&*b.store, "reclaim/notabucket/x#u", b"{}");
        put(&*b.store, "reclaim/stray", b"{}");
        let seen = RefCell::new(Vec::new());
        let taken = q
            .drain(NOW + 60_000, &|_| true, &mut |i| {
                seen.borrow_mut().push(i.name.to_string());
                Err(MetaError::Contended)
            })
            .unwrap();
        // Neither the corrupt nor the oversized body reaches the reclaimer.
        assert_eq!(seen.into_inner(), ["fails"], "{:?}", b.kind);
        assert_eq!(taken, 2);
        assert_eq!(
            keys(&*b.store, "reclaim/"),
            [
                format!("reclaim/{due}/fails#u"),
                "reclaim/notabucket/x#u".to_owned(),
                "reclaim/stray".to_owned(),
            ]
        );
    }
}

#[test]
fn a_bucket_this_node_does_not_own_is_left_to_its_owner() {
    for b in all() {
        let q = queue(&b.store).with_delay_millis(0);
        q.enqueue(&intent("theirs", "u", false), NOW).unwrap();
        q.enqueue(&intent("mine", "u", false), NOW + 60_000)
            .unwrap();
        let theirs = bucket(NOW);
        let seen = RefCell::new(Vec::new());
        let (taken, n) = counted(b.meter(), || {
            q.drain(NOW + 120_000, &|bucket| bucket != theirs, &mut |i| {
                seen.borrow_mut().push(i.name.to_string());
                Ok(())
            })
        });
        assert_eq!(taken.unwrap(), 1);
        assert_eq!(seen.into_inner(), ["mine"]);
        assert_eq!(n, vec![(Get, 1), (List, 1), (Delete, 1)], "{:?}", b.kind);
        assert_eq!(
            keys(&*b.store, "reclaim/"),
            [format!("reclaim/{theirs}/theirs#u")]
        );
    }
}

#[test]
fn a_drain_pages_through_a_thousand_due_intents_and_more() {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(64 << 20, 1));
    let q = queue(&store).with_delay_millis(0);
    for i in 0..1001 {
        let name: &'static str = format!("i{i:04}").leak();
        q.enqueue(&intent(name, "u", false), NOW).unwrap();
    }
    let ((taken, seen), n) = counted(store.meter(), || drain_all(&q, NOW + 60_000));
    assert_eq!(taken, 1001);
    assert_eq!(seen.len(), 1001);
    assert_eq!(n, vec![(Get, 1001), (List, 2), (Delete, 1001)]);
    assert!(keys(&*store, "reclaim/").is_empty());
}

#[test]
fn a_drain_lists_nothing_past_the_first_bucket_not_yet_due() {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(64 << 20, 1));
    let q = queue(&store).with_delay_millis(0);
    q.enqueue(&intent("due", "u", false), NOW).unwrap();
    for i in 0..1000 {
        let name: &'static str = format!("l{i:04}").leak();
        q.enqueue(&intent(name, "u", false), NOW + 120_000).unwrap();
    }
    // The first page holds the due intent and 999 later ones, and claims
    // more: the walk ends at the first later one, with no second LIST.
    let ((taken, _), n) = counted(store.meter(), || drain_all(&q, NOW + 60_000));
    assert_eq!(taken, 1);
    assert_eq!(n, vec![(Get, 1), (List, 1), (Delete, 1)]);
}

#[test]
fn a_drain_moves_past_a_thousand_kept_intents() {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(64 << 20, 1));
    let q = queue(&store).with_delay_millis(0);
    for i in 0..1001 {
        let name: &'static str = format!("k{i:04}").leak();
        q.enqueue(&intent(name, "u", false), NOW).unwrap();
    }
    q.enqueue(&intent("next", "u", false), NOW + 60_000)
        .unwrap();
    let mut calls = 0;
    let (taken, n) = counted(store.meter(), || {
        q.drain(NOW + 120_000, &|_| true, &mut |i| {
            calls += 1;
            assert!(calls <= 1002, "the walk relisted what it kept");
            if i.name == "next" {
                Ok(())
            } else {
                Err(MetaError::Contended)
            }
        })
    });
    assert_eq!(taken.unwrap(), 1);
    assert_eq!(n, vec![(Get, 1002), (List, 2), (Delete, 1)]);
}

#[test]
fn a_page_ending_in_another_nodes_bucket_skips_the_rest_of_it() {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(64 << 20, 1));
    let q = queue(&store).with_delay_millis(0);
    for i in 0..2500 {
        let name: &'static str = format!("t{i:04}").leak();
        q.enqueue(&intent(name, "u", false), NOW).unwrap();
    }
    q.enqueue(&intent("mine", "u", false), NOW + 60_000)
        .unwrap();
    let theirs = bucket(NOW);
    let asked = std::cell::Cell::new(0);
    let owns = |b: &str| {
        asked.set(asked.get() + 1);
        assert!(asked.get() <= 5000, "the walk relisted their bucket");
        b != theirs
    };
    let (taken, n) = counted(store.meter(), || {
        q.drain(NOW + 120_000, &owns, &mut |i| {
            assert_eq!(i.name, "mine");
            Ok(())
        })
    });
    assert_eq!(taken.unwrap(), 1);
    // One page of theirs, then one starting past their bucket.
    assert_eq!(n, vec![(Get, 1), (List, 2), (Delete, 1)]);
}

#[test]
fn a_failed_get_keeps_the_intent() {
    // Request 1 is the enqueue, 2 the LIST, 3 the GET.
    let store: Arc<dyn BlobStore> = Arc::new(Quirky::new(Quirk::FailRequest(3)));
    let q = queue(&store).with_delay_millis(0);
    q.enqueue(&intent("logs", "u", false), NOW).unwrap();
    assert_eq!(drain_all(&q, NOW + 60_000), (0, vec![]));
    assert_eq!(drain_all(&q, NOW + 60_000), (1, vec!["logs".to_owned()]));
}

fn recreated(d: &DescriptorStore, store: &dyn BlobStore) {
    let mut buf = [0u8; 4096];
    d.create(&IndexRecord::new("logs", "u1", 1), NOW, &mut buf)
        .unwrap();
    d.delete("logs").unwrap();
    d.create(&IndexRecord::new("logs", "u2", 1), NOW, &mut buf)
        .unwrap();
    shard_blob(store, "logs", "u1", 0, "manifest");
    shard_blob(store, "logs", "u2", 0, "manifest");
}

#[test]
fn a_reclaim_removes_only_the_dead_incarnation() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        recreated(&d, &*b.store);
        // No writer then and no head now: nothing was left to purge.
        let heads = Heads::default();
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        let (done, n) = counted(b.meter(), || l.reclaim(&intent("logs", "u1", false)));
        done.unwrap();
        assert_eq!(n, vec![(Get, 1)], "{:?}", b.kind);
        assert_eq!(heads.calls(), ["delete all logs 1 u1"]);
        assert_eq!(keys(&*b.store, "segments/").len(), 2);
        // A head that reappeared: a writer may have published since.
        let heads = Heads {
            reappeared: 1,
            ..Heads::default()
        };
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.reclaim(&intent("logs", "u1", false)).unwrap();
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u2#0/manifest"]
        );
        // A writer at the delete: purged whatever the heads say.
        shard_blob(&*b.store, "logs", "u1", 0, "late");
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        l.reclaim(&intent("logs", "u1", true)).unwrap();
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u2#0/manifest"]
        );
        // The live incarnation's own intent touches nothing.
        let heads = Heads::default();
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.reclaim(&intent("logs", "u2", true)).unwrap();
        assert!(heads.calls().is_empty());
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u2#0/manifest"]
        );
    }
}

#[test]
fn a_deleted_writers_bytes_go_when_the_intent_falls_due() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let heads = Heads::owned(0);
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        l.delete_index("logs", None, NOW).unwrap().unwrap();
        // The writer that missed the delete publishes once more.
        shard_blob(&*b.store, "logs", "u1", 0, "t=2/_1.cfs");
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        let early = q
            .drain(NOW + 60_000, &|_| true, &mut |i| l.reclaim(i))
            .unwrap();
        assert_eq!(early, 0);
        let taken = q
            .drain(NOW + 180_000, &|_| true, &mut |i| l.reclaim(i))
            .unwrap();
        assert_eq!(taken, 1, "{:?}", b.kind);
        assert!(keys(&*b.store, "segments/").is_empty());
        assert!(keys(&*b.store, "reclaim/").is_empty());
    }
}

#[test]
fn a_purge_pages_through_a_large_shard_and_counts_a_failure() {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(64 << 20, 1));
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let q = queue(&store);
    let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
    l.create_index(&IndexRecord::new("big", "u1", 1), NOW)
        .unwrap();
    for i in 0..1001 {
        shard_blob(&*store, "big", "u1", 0, &format!("f{i:04}"));
    }
    let (deleted, n) = counted(store.meter(), || l.delete_index("big", None, NOW));
    assert_eq!(deleted.unwrap().unwrap().purged, Some(Purged::default()));
    assert_eq!(n, vec![(Get, 1), (Put, 1), (List, 2), (Delete, 1002)]);
    assert!(keys(&*store, "segments/").is_empty());

    // Requests: 1 create, 2 read, 3 intent, 4 delete, 5 the shard's LIST.
    let store: Arc<dyn BlobStore> = Arc::new(Quirky::new(Quirk::FailRequest(5)));
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let q = queue(&store);
    let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
    l.create_index(&IndexRecord::new("logs", "u1", 2), NOW)
        .unwrap();
    let deleted = l.delete_index("logs", None, NOW).unwrap().unwrap();
    assert_eq!(
        deleted.purged,
        Some(Purged {
            pinned: 0,
            failed: 1
        })
    );
}
