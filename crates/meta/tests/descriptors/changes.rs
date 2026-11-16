//! AC-2 to AC-4 of docs/specs/change-stripes/SPEC.md: change stripes.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_blob::S3Op::{Delete, Get, GetRange, Put};
use oss_format::IndexRecord;
use oss_meta::ChangeKind::{Changed, Deleted, PendingDelete};
use oss_meta::{ChangeStripes, DeleteMode, DescriptorStore, MetaError, STRIPES, Snapshot};

use crate::probe::{Quirk, Quirky};
use crate::{BASE, Backend, all, counted};

const NOW: i64 = 1_763_164_800_000;

fn stripes(b: &Backend) -> ChangeStripes {
    ChangeStripes::new(Arc::clone(&b.store), BASE).unwrap()
}

/// A name in `name`'s stripe other than `name`.
fn neighbour(name: &str) -> String {
    let stripe = ChangeStripes::stripe_of(name);
    (0..)
        .map(|i| format!("other-{i}"))
        .find(|other| ChangeStripes::stripe_of(other) == stripe)
        .unwrap()
}

fn read(s: &ChangeStripes, name: &str) -> Snapshot {
    s.read(ChangeStripes::stripe_of(name)).unwrap()
}

#[test]
fn a_change_to_another_index_in_the_stripe_leaves_an_index_confirmed() {
    for b in all() {
        let s = stripes(&b);
        let other = neighbour("alpha");
        let before = read(&s, "alpha");
        assert_eq!(
            (before.epoch, before.generation, before.oldest_kept),
            (0, 0, 1)
        );
        s.bump(&other, Changed).unwrap();
        let after = read(&s, "alpha");
        assert!(after.unchanged_since("alpha", before.epoch, before.generation));
        assert!(!after.unchanged_since(&other, before.epoch, before.generation));
        assert!(after.unchanged_since(&other, after.epoch, after.generation));
        // A read from a later generation, or another epoch, proves nothing.
        assert!(!before.unchanged_since("alpha", after.epoch, after.generation));
        assert!(!after.unchanged_since("alpha", after.epoch + 1, after.generation));
    }
}

#[test]
fn an_unfinished_delete_is_never_confirmed_until_it_finishes() {
    for b in all() {
        let s = stripes(&b);
        s.bump("alpha", PendingDelete).unwrap();
        let during = read(&s, "alpha");
        assert!(!during.unchanged_since("alpha", during.epoch, during.generation));
        let other = neighbour("alpha");
        for _ in 0..300 {
            s.bump(&other, Changed).unwrap();
        }
        let later = read(&s, "alpha");
        assert!(later.pending.contains("alpha"), "{:?}", b.kind);
        assert!(!later.unchanged_since("alpha", later.epoch, later.generation));
        s.bump("alpha", Deleted).unwrap();
        let finished = read(&s, "alpha");
        assert!(finished.unchanged_since("alpha", finished.epoch, finished.generation));
        // But the delete itself is a change.
        assert!(!finished.unchanged_since("alpha", later.epoch, later.generation));
    }
}

#[test]
fn an_update_does_not_finish_a_delete() {
    for b in all() {
        let s = stripes(&b);
        s.bump("alpha", PendingDelete).unwrap();
        s.bump("alpha", Changed).unwrap();
        let r = read(&s, "alpha");
        assert!(!r.unchanged_since("alpha", r.epoch, r.generation));
        assert_eq!(r.changed["alpha"], 2);
    }
}

#[test]
fn a_read_older_than_every_kept_change_is_not_confirmed() {
    for b in all() {
        let s = stripes(&b);
        let other = neighbour("alpha");
        let before = read(&s, "alpha");
        for _ in 0..300 {
            s.bump(&other, Changed).unwrap();
        }
        let after = read(&s, "alpha");
        assert_eq!(after.oldest_kept, 300 - 256 + 1, "{:?}", b.kind);
        assert!(!after.unchanged_since("alpha", before.epoch, before.generation));
        // A read inside what is kept is still confirmed.
        assert!(after.unchanged_since("alpha", after.epoch, after.oldest_kept - 1));
        // One generation older, and the change at oldest_kept - 1 is gone.
        assert!(!after.unchanged_since("alpha", after.epoch, after.oldest_kept - 2));
    }
}

#[test]
fn too_many_unfinished_deletes_start_a_new_epoch() {
    for b in all() {
        let s = stripes(&b);
        let stripe = ChangeStripes::stripe_of("alpha");
        let before = s.read(stripe).unwrap();
        let gone: Vec<String> = (0..)
            .map(|i| format!("gone-{i}"))
            .filter(|n| ChangeStripes::stripe_of(n) == stripe)
            .take(65)
            .collect();
        for name in &gone {
            s.bump(name, PendingDelete).unwrap();
        }
        let after = s.read(stripe).unwrap();
        assert_eq!((after.epoch, after.generation), (1, 65), "{:?}", b.kind);
        assert!(after.pending.is_empty() && after.changed.is_empty());
        assert!(!after.unchanged_since("alpha", before.epoch, before.generation));
    }
}

#[test]
fn a_bump_is_a_read_and_a_swap() {
    for b in all() {
        let s = stripes(&b);
        let (_, sent) = counted(b.meter(), || s.bump("alpha", Changed).unwrap());
        assert_eq!(sent, [(Get, 1), (Put, 1)], "{:?} create", b.kind);
        let (_, sent) = counted(b.meter(), || s.bump("alpha", Changed).unwrap());
        assert_eq!(
            sent,
            b.budget(&[(Get, 1), (Put, 1)], &[(Get, 1), (GetRange, 1), (Put, 1)]),
            "{:?} swap",
            b.kind
        );
        let (_, sent) = counted(b.meter(), || read(&s, "alpha"));
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}

#[test]
fn a_bump_retries_while_others_move_the_stripe() {
    let store = Arc::new(Quirky::new(Quirk::ConflictSwaps(3)));
    let s = ChangeStripes::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE).unwrap();
    let (_, sent) = counted(store.meter(), || s.bump("alpha", Changed).unwrap());
    // Three lost attempts and the one that lands: each a read and a swap,
    // the lost ones refused before they reach the store.
    assert_eq!(sent, [(Get, 4), (Put, 1)]);
    assert_eq!(read(&s, "alpha").generation, 1);

    let store = Arc::new(Quirky::new(Quirk::ConflictSwaps(u32::MAX)));
    let s = ChangeStripes::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE).unwrap();
    let (got, sent) = counted(store.meter(), || s.bump("alpha", Changed));
    assert!(matches!(got, Err(MetaError::Contended)), "{got:?}");
    assert_eq!(sent, [(Get, 64)]);
}

#[test]
fn a_bad_name_or_stripe_sends_nothing() {
    for b in all() {
        let s = stripes(&b);
        let (got, sent) = counted(b.meter(), || {
            [
                s.bump(&"a".repeat(256), Changed),
                s.bump("a\nb", Changed),
                s.read(STRIPES).map(|_| ()),
            ]
        });
        assert!(sent.is_empty(), "{:?} {sent:?}", b.kind);
        let [long, newline, stripe] = got;
        assert!(matches!(long, Err(MetaError::InvalidArgument(_))));
        assert!(matches!(newline, Err(MetaError::InvalidArgument(_))));
        assert!(matches!(stripe, Err(MetaError::Format(_))));
        // The longest name is taken.
        s.bump(&"a".repeat(255), Changed).unwrap();
    }
    assert!(ChangeStripes::new(Arc::clone(&all()[0].store), "d").is_err());
}

#[test]
fn a_stripe_the_java_wrote_is_read() {
    for b in all() {
        let s = stripes(&b);
        let bytes = include_bytes!("../../../../fixtures/change-stripes/bumps.txt");
        let stripe = ChangeStripes::stripe_of("alpha");
        b.store
            .create_register(&format!("{BASE}index-changes/stripe-{stripe:02}"), bytes)
            .unwrap();
        let r = s.read(stripe).unwrap();
        assert_eq!((r.epoch, r.generation, r.oldest_kept), (0, 6, 1));
        assert_eq!(r.pending.iter().collect::<Vec<_>>(), ["alpha"]);
        assert_eq!(r.changed["beta"], 6);
        // And a bump over it keeps the Java's entries.
        s.bump("alpha", Deleted).unwrap();
        let r = s.read(stripe).unwrap();
        assert!(r.pending.is_empty());
        assert_eq!((r.generation, r.changed["alpha"]), (7, 7));
        // A malformed one is refused, by a read and a bump alike.
        let other = (stripe + 1) % STRIPES;
        b.store
            .create_register(
                &format!("{BASE}index-changes/stripe-{other:02}"),
                b"v2 0 0\n",
            )
            .unwrap();
        assert!(matches!(s.read(other), Err(MetaError::Format(_))));
    }
}

/// A descriptor store recording its changes, and the stripes it records in.
fn recording(b: &Backend, deletes: DeleteMode) -> (DescriptorStore, ChangeStripes) {
    let d = b.descriptors(deletes).record_changes_in(stripes(b));
    (d, stripes(b))
}

#[test]
fn an_applied_update_is_recorded_and_a_refused_one_is_not() {
    for b in all() {
        let (d, s) = recording(&b, DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let record = IndexRecord::new("alpha", "u", 1);
        let g = d.create(&record, NOW, &mut buf).unwrap();
        assert_eq!(read(&s, "alpha").generation, 0, "a create is not recorded");
        let (moved, sent) = counted(b.meter(), || d.update(&record, g, &mut buf).unwrap());
        assert!(moved.is_some());
        assert_eq!(
            sent,
            b.budget(&[(Get, 1), (Put, 2)], &[(Get, 1), (GetRange, 1), (Put, 2)]),
            "{:?}",
            b.kind
        );
        let r = read(&s, "alpha");
        assert_eq!((r.generation, r.changed["alpha"]), (1, 1));
        assert_eq!(d.update(&record, g, &mut buf).unwrap(), None);
        assert_eq!(read(&s, "alpha").generation, 1, "{:?}", b.kind);
    }
}

#[test]
fn a_delete_is_recorded_pending_then_finished() {
    for deletes in [DeleteMode::Tombstone, DeleteMode::Conditional] {
        for b in all() {
            let (d, s) = recording(&b, deletes);
            let mut buf = vec![0u8; 4096];
            let g = d
                .create(&IndexRecord::new("alpha", "u", 1), NOW, &mut buf)
                .unwrap();
            let (deleted, sent) = counted(b.meter(), || {
                d.delete_if_unchanged("alpha", g, NOW).unwrap()
            });
            assert!(deleted);
            // The delete's own requests, and two bumps: a create of the
            // stripe, then a swap.
            let expected = match (deletes, b.kind == crate::Kind::Fs) {
                (DeleteMode::Tombstone, false) => vec![(Get, 2), (Put, 4)],
                (DeleteMode::Tombstone, true) => vec![(Get, 2), (GetRange, 2), (Put, 4)],
                (DeleteMode::Conditional, false) => vec![(Get, 2), (Put, 2), (Delete, 1)],
                (DeleteMode::Conditional, true) => {
                    vec![(Get, 2), (GetRange, 2), (Put, 2), (Delete, 1)]
                }
            };
            assert_eq!(sent, expected, "{deletes:?} {:?}", b.kind);
            let r = read(&s, "alpha");
            assert_eq!((r.generation, r.changed["alpha"]), (2, 2), "{:?}", b.kind);
            assert!(r.pending.is_empty());
            // A refused delete is an attempt over, too.
            assert!(!d.delete_if_unchanged("alpha", g, NOW).unwrap());
            assert_eq!(read(&s, "alpha").generation, 4);
            // And the unconditional delete: two bumps around it.
            let (_, sent) = counted(b.meter(), || d.delete("alpha").unwrap());
            assert_eq!(
                sent,
                b.budget(
                    &[(Get, 2), (Put, 2), (Delete, 1)],
                    &[(Get, 2), (GetRange, 2), (Put, 2), (Delete, 1)]
                ),
                "{deletes:?} {:?}",
                b.kind
            );
            let r = read(&s, "alpha");
            assert_eq!(r.generation, 6);
            assert!(r.pending.is_empty());
        }
    }
}

/// A recording store over a wrapper whose quirk fails one request.
fn recording_quirky(quirk: Quirk) -> (DescriptorStore, Arc<Quirky>) {
    let store = Arc::new(Quirky::new(quirk));
    let shared = Arc::clone(&store) as Arc<dyn BlobStore>;
    let d = DescriptorStore::new(Arc::clone(&shared), BASE, DeleteMode::Tombstone)
        .unwrap()
        .record_changes_in(ChangeStripes::new(shared, BASE).unwrap());
    (d, store)
}

#[test]
fn a_delete_whose_pending_entry_fails_is_refused() {
    // The create, then the pending bump's read.
    let (d, store) = recording_quirky(Quirk::FailRequest(2));
    let g = d
        .create(
            &IndexRecord::new("alpha", "u", 1),
            NOW,
            &mut vec![0u8; 4096],
        )
        .unwrap();
    assert!(matches!(
        d.delete_if_unchanged("alpha", g, NOW),
        Err(MetaError::Blob(_))
    ));
    let mut buf = vec![0u8; 4096];
    assert!(
        d.get("alpha", &mut buf).unwrap().is_some(),
        "the index is untouched"
    );
    assert_eq!(
        store
            .inner
            .read_register(&format!("{BASE}indices/alpha"), &mut buf)
            .unwrap()
            .unwrap()
            .generation,
        g
    );
    // So is the unconditional delete.
    let (d, _) = recording_quirky(Quirk::FailRequest(2));
    d.create(&IndexRecord::new("alpha", "u", 1), NOW, &mut buf)
        .unwrap();
    assert!(matches!(d.delete("alpha"), Err(MetaError::Blob(_))));
    assert!(d.get("alpha", &mut buf).unwrap().is_some());
}

#[test]
fn a_delete_whose_finished_entry_fails_still_answers() {
    // The create; the pending bump's read and create; the marker and the
    // swap to a tombstone; then the finished bump's read.
    let (d, store) = recording_quirky(Quirk::FailRequest(6));
    let g = d
        .create(
            &IndexRecord::new("alpha", "u", 1),
            NOW,
            &mut vec![0u8; 4096],
        )
        .unwrap();
    assert!(d.delete_if_unchanged("alpha", g, NOW).unwrap());
    let s = ChangeStripes::new(Arc::clone(&store.inner), BASE).unwrap();
    // Left pending, so the stripe never confirms the name: safe.
    assert!(read(&s, "alpha").pending.contains("alpha"));
    // An update whose record fails still answers too: the create, the
    // update's swap, then its bump's read.
    let (d, _) = recording_quirky(Quirk::FailRequest(3));
    let mut buf = vec![0u8; 4096];
    let record = IndexRecord::new("alpha", "u", 1);
    let g = d.create(&record, NOW, &mut buf).unwrap();
    assert!(d.update(&record, g, &mut buf).unwrap().is_some());
    // The unconditional delete as well: the create, the pending bump's
    // read and create, the delete, then the finished bump's read.
    let (d, _) = recording_quirky(Quirk::FailRequest(5));
    d.create(&record, NOW, &mut buf).unwrap();
    d.delete("alpha").unwrap();
    assert!(d.get("alpha", &mut buf).unwrap().is_none());
}

#[test]
fn the_stripe_is_stored_as_the_javas_text() {
    for b in all() {
        stripes(&b).bump("alpha", Changed).unwrap();
        let stripe = ChangeStripes::stripe_of("alpha");
        let mut buf = vec![0u8; 64];
        let got = b
            .store
            .read_register(&format!("{BASE}index-changes/stripe-{stripe:02}"), &mut buf)
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..got.len], b"v1 0 1\n1 D alpha\n");
    }
}

#[test]
fn a_failed_delete_is_still_finished_in_its_stripe() {
    // The create; the pending bump's read and create; the marker; then
    // the swap to a tombstone fails.
    let (d, store) = recording_quirky(Quirk::FailRequest(5));
    let g = d
        .create(
            &IndexRecord::new("alpha", "u", 1),
            NOW,
            &mut vec![0u8; 4096],
        )
        .unwrap();
    assert!(matches!(
        d.delete_if_unchanged("alpha", g, NOW),
        Err(MetaError::Blob(_))
    ));
    let s = ChangeStripes::new(Arc::clone(&store.inner), BASE).unwrap();
    let r = read(&s, "alpha");
    assert!(r.pending.is_empty());
    assert_eq!(r.generation, 2);
}

#[test]
fn a_name_no_stripe_holds_is_not_recorded() {
    // A filesystem refuses a key segment past 255 bytes on its own.
    for b in all().into_iter().filter(|b| b.kind != crate::Kind::Fs) {
        let (d, s) = recording(&b, DeleteMode::Tombstone);
        let long = "a".repeat(256);
        // Deleting an absent register stays idempotent, and records nothing.
        d.delete(&long).unwrap();
        assert_eq!(
            s.read(ChangeStripes::stripe_of(&long)).unwrap().generation,
            0
        );
    }
}
