//! The purge modes of docs/specs/index-delete-purge/SPEC.md: a purge
//! deferred above a shard count, a purge filed again while a pin holds, and
//! a purge that deletes a page of blobs in one request.

use oss_blob::S3Op::Delete;
use oss_format::IndexRecord;
use oss_meta::{DeleteMode, Deleted};

use crate::alias_mutations::NOW;
use crate::index_lifecycle::{NO_PINS, NoHeads, Pins, keys, lifecycle, queue, shard_blob};
use crate::{Kind, all, counted};

fn intents(store: &dyn oss_blob::BlobStore) -> Vec<String> {
    keys(store, "reclaim/")
}

#[test]
fn an_index_above_the_inline_limit_is_purged_at_the_reclaim() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS).with_inline_purge_limit(2);
        l.create_index(&IndexRecord::new("logs", "u1", 3), NOW)
            .unwrap();
        for shard in 0..3 {
            shard_blob(&*b.store, "logs", "u1", shard, "manifest");
        }
        let deleted = l.delete_index("logs", None, NOW).unwrap();
        assert_eq!(
            deleted,
            Some(Deleted {
                uuid: "u1".into(),
                bucket: None,
                aliases_left: 0,
                purged: None
            }),
            "{:?}",
            b.kind
        );
        assert_eq!(keys(&*b.store, "segments/").len(), 3);
        let mut taken = Vec::new();
        q.drain(NOW + 180_000, &|_| true, &mut |i| {
            taken.push(i.had_writer);
            l.reclaim(i)
        })
        .unwrap();
        assert_eq!(taken, [true]);
        assert!(keys(&*b.store, "segments/").is_empty());
    }
}

#[test]
fn an_index_within_the_inline_limit_is_purged_inline() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS).with_inline_purge_limit(3);
        l.create_index(&IndexRecord::new("logs", "u1", 3), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        let deleted = l.delete_index("logs", None, NOW).unwrap().unwrap();
        assert!(deleted.purged.is_some());
        assert!(keys(&*b.store, "segments/").is_empty());
    }
}

#[test]
fn a_pinned_shard_is_purged_once_its_pin_ends() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let pinned = Pins(Some(vec![0]));
        let mut l = lifecycle(&d, &q, &NoHeads, &pinned).with_repurge_pinned(true);
        l.create_index(&IndexRecord::new("logs", "u1", 2), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        shard_blob(&*b.store, "logs", "u1", 1, "manifest");
        l.delete_index("logs", None, NOW).unwrap().unwrap();
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u1#0/manifest"]
        );
        // The first intent finds nothing to do; the pin still holds.
        let mut l = lifecycle(&d, &q, &NoHeads, &pinned).with_repurge_pinned(true);
        let mut at = NOW + 180_000;
        q.drain(at, &|_| true, &mut |i| l.reclaim_at(i, at))
            .unwrap();
        assert_eq!(keys(&*b.store, "segments/").len(), 1, "{:?}", b.kind);
        assert!(!intents(&*b.store).is_empty(), "filed again while pinned");
        // The pin ends: the next reclaim purges and files nothing more.
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS).with_repurge_pinned(true);
        at += 300_000;
        q.drain(at, &|_| true, &mut |i| l.reclaim_at(i, at))
            .unwrap();
        assert!(keys(&*b.store, "segments/").is_empty(), "{:?}", b.kind);
        assert!(intents(&*b.store).is_empty());
    }
}

#[test]
fn a_pinned_shard_is_left_as_the_java_does_by_default() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let pinned = Pins(Some(vec![0]));
        let mut l = lifecycle(&d, &q, &NoHeads, &pinned);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        l.delete_index("logs", None, NOW).unwrap().unwrap();
        q.drain(NOW + 180_000, &|_| true, &mut |i| l.reclaim_at(i, NOW))
            .unwrap();
        assert!(intents(&*b.store).is_empty(), "{:?}", b.kind);
    }
}

#[test]
fn a_shards_blobs_are_deleted_in_one_request() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        for file in ["a", "b", "c"] {
            shard_blob(&*b.store, "logs", "u1", 0, file);
        }
        let (done, n) = counted(b.meter(), || l.delete_index("logs", None, NOW));
        done.unwrap().unwrap();
        // The descriptor, then one batch for the three blobs; a store with
        // no batch delete sends one a blob.
        let expected = if b.kind == Kind::Fs { 4 } else { 2 };
        let deletes = n.iter().find(|(op, _)| *op == Delete).map(|(_, c)| *c);
        assert_eq!(deletes, Some(expected), "{:?} {n:?}", b.kind);
    }
}

/// The `had_writer` of the intents filed so far, taking them.
fn filed(q: &oss_meta::ReclaimQueue) -> Vec<bool> {
    let mut seen = Vec::new();
    q.drain(NOW + 180_000, &|_| true, &mut |i| {
        seen.push(i.had_writer);
        Ok(())
    })
    .unwrap();
    seen
}

#[test]
fn a_delete_files_one_intent_and_only_a_pin_files_another() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        for (pins, repurge, expect) in [
            (&NO_PINS, true, false),
            (&Pins(Some(vec![0])), false, false),
            (&Pins(Some(vec![0])), true, true),
        ] {
            let q = queue(&b.store);
            let name = format!("i{}{}", repurge, expect);
            let mut l = lifecycle(&d, &q, &NoHeads, pins).with_repurge_pinned(repurge);
            l.create_index(&IndexRecord::new(&name, "u1", 1), NOW)
                .unwrap();
            shard_blob(&*b.store, &name, "u1", 0, "manifest");
            l.delete_index(&name, None, NOW).unwrap().unwrap();
            assert_eq!(
                filed(&q),
                [expect],
                "{:?} repurge {repurge} pinned {}",
                b.kind,
                pins.0.is_some()
            );
        }
    }
}
