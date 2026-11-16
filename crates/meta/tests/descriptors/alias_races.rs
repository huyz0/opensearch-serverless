//! docs/specs/alias-mutations/SPEC.md, AC-3 and the deadline in AC-2:
//! another writer's step planted just before a chosen request.

use std::sync::{Arc, Mutex};

use oss_blob::S3Op::Get;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::descriptor_key;
use oss_meta::{
    AliasClock, AliasMutations, AliasRecord, DeleteMode, DescriptorStore, MetaError, Target,
};

use crate::alias_mutations::{
    NOW, TTL, at_now, crashed, hints, index, mutations, names, no_pause, raw,
};
use crate::probe::{Quirk, Quirky};
use crate::{BASE, all, counted};

/// A descriptor store over a store that runs `hook` once armed.
fn hooked(hook: impl Fn(&dyn BlobStore) + Send + Sync + 'static) -> (DescriptorStore, Arc<Quirky>) {
    let quirky = Arc::new(Quirky::hooked(hook));
    let d = DescriptorStore::new(quirky.clone(), BASE, DeleteMode::Tombstone).unwrap();
    (d, quirky)
}

fn generation(d: &DescriptorStore, name: &str) -> u64 {
    d.generation_of(name, &mut [0u8; 4096]).unwrap()
}

#[test]
fn a_creation_removed_mid_preparation_is_superseded() {
    let key = descriptor_key(BASE, "al").unwrap();
    let (d, quirky) = hooked(move |inner| inner.delete(&key).unwrap());
    index(&d, "a");
    let before = generation(&d, "a");
    let mut m = mutations(&d, &at_now);
    // The alias's read, its reservation, the read of `a`, then the
    // reservation's read before `a`'s swap: another writer removed the
    // reservation just before it.
    quirky.arm(4);
    let got = m.create_alias(&AliasRecord::new("al", &["a"]));
    assert!(matches!(got, Err(MetaError::Superseded)), "{got:?}");
    assert_eq!(raw(&d, "al"), None);
    // The abort swaps `a` though its hint was never added, so a preparation
    // that read `a` before cannot land.
    assert_ne!(generation(&d, "a"), before);
    assert!(hints(&d, "a").is_empty());
}

#[test]
fn a_reservation_taken_over_mid_preparation_is_superseded() {
    let key = descriptor_key(BASE, "al").unwrap();
    // Another writer swaps the reservation, which stays pending at a new
    // generation.
    let (d, quirky) = hooked(move |inner| {
        let mut buf = [0u8; 4096];
        let read = inner.read_register(&key, &mut buf).unwrap().unwrap();
        let value = buf[..read.len].to_vec();
        inner.swap_register(&key, read.generation, &value).unwrap();
    });
    index(&d, "a");
    index(&d, "b");
    let mut m = mutations(&d, &at_now);
    let before = AliasRecord::new("al", &["a"]);
    let g = m.create_alias(&before).unwrap();
    // The reservation's swap, the read of `a`, then the reservation's read.
    quirky.arm(3);
    let got = m.update_alias(&before, &AliasRecord::new("al", &["a", "b"]), g);
    assert!(matches!(got, Err(MetaError::Superseded)), "{got:?}");
    // The abort's restore lost to the takeover, which stands.
    let now = raw(&d, "al").unwrap();
    assert!(now.alias.mutation_pending());
    assert_ne!(now.generation, g);
}

#[test]
fn a_failed_preparation_undoes_its_reservation() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    index(&d, "a");
    index(&d, "b");
    let mut m = mutations(&d, &at_now);
    let before = AliasRecord::new("al", &["a"]);
    let g = m.create_alias(&before).unwrap();
    // `b` exists but `c` does not: `b`'s hint is prepared, then undone.
    let got = m.update_alias(&before, &AliasRecord::new("al", &["a", "b", "c"]), g);
    assert!(matches!(&got, Err(MetaError::MissingMember(n)) if n == "c"));
    // The abort republished the record: a mutation adding only `c` fails too.
    let g = raw(&d, "al").unwrap().generation;
    let got = m.update_alias(&before, &AliasRecord::new("al", &["a", "c"]), g);
    assert!(
        matches!(&got, Err(MetaError::MissingMember(n)) if n == "c"),
        "{got:?}"
    );
    let now = raw(&d, "al").unwrap().alias;
    assert_eq!(now, before);
    assert!(hints(&d, "b").is_empty());
    assert_eq!(hints(&d, "a"), ["al"]);
}

#[test]
fn a_store_failure_mid_mutation_is_reported_and_undone() {
    // Request 1 creates `a`; at 5 the reservation's read before `a`'s
    // swap fails.
    let quirky = Arc::new(Quirky::new(Quirk::FailRequest(5)));
    let d = DescriptorStore::new(quirky, BASE, DeleteMode::Tombstone).unwrap();
    index(&d, "a");
    let mut m = mutations(&d, &at_now);
    let got = m.create_alias(&AliasRecord::new("al", &["a"]));
    assert!(matches!(got, Err(MetaError::Blob(_))), "{got:?}");
    assert_eq!(raw(&d, "al"), None);
    assert!(hints(&d, "a").is_empty());
}

#[test]
fn a_reservation_deadline_saturates() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    index(&d, "a");
    let far = || i64::MAX - 1;
    let mut m = mutations(&d, &far);
    let alias = AliasRecord::new("al", &["a"]);
    let g = m.create_alias(&alias).unwrap();
    let late = crashed(&d, &alias, g, &[], false, i64::MAX);
    let read = m.resolve_for_mutation("al").unwrap().unwrap();
    // Not yet past a deadline of i64::MAX: left pending.
    assert_eq!(read.generation, late);
    // A clock at the epoch, with no lease, still stamps a deadline above
    // zero, which a reservation needs.
    let epoch = || 0;
    let mut early = AliasMutations::new(
        &d,
        AliasClock {
            now_millis: &epoch,
            pause: &no_pause,
        },
        0,
        4096,
    );
    early.create_alias(&AliasRecord::new("b0", &["a"])).unwrap();
}

#[test]
fn a_reservation_carries_its_deadline() {
    let key = descriptor_key(BASE, "al").unwrap();
    let seen = Arc::new(Mutex::new(None));
    let into = seen.clone();
    let (d, quirky) = hooked(move |inner| {
        let mut buf = [0u8; 4096];
        let read = inner.read_register(&key, &mut buf).unwrap().unwrap();
        let alias = AliasRecord::parse(&buf[..read.len]).unwrap();
        *into.lock().unwrap() = Some(alias.mutation_until());
    });
    index(&d, "a");
    let mut m = mutations(&d, &at_now);
    // The alias's read, its reservation, then the read of `a`.
    quirky.arm(3);
    m.create_alias(&AliasRecord::new("al", &["a"])).unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(NOW + TTL));
}

#[test]
fn a_hint_is_kept_while_the_alias_names_its_index_again() {
    let key = descriptor_key(BASE, "al").unwrap();
    // Another writer puts `a` back into the alias.
    let (d, quirky) = hooked(move |inner| {
        let mut buf = [0u8; 4096];
        let read = inner.read_register(&key, &mut buf).unwrap().unwrap();
        let len = AliasRecord::new("al", &["a", "b"]).write(&mut buf).unwrap();
        let value = buf[..len].to_vec();
        inner.swap_register(&key, read.generation, &value).unwrap();
    });
    index(&d, "a");
    index(&d, "b");
    let mut m = mutations(&d, &at_now);
    let before = AliasRecord::new("al", &["a", "b"]);
    let g = m.create_alias(&before).unwrap();
    // The reservation, the publishing swap, the read of `a`, then the
    // alias's read before `a`'s hint is removed.
    quirky.arm(4);
    m.update_alias(&before, &AliasRecord::new("al", &["b"]), g)
        .unwrap()
        .unwrap();
    assert_eq!(raw(&d, "al").unwrap().alias.indices, ["a", "b"]);
    assert_eq!(hints(&d, "a"), ["al"], "a member keeps its hint");
}

#[test]
fn an_alias_that_keeps_changing_is_counted_as_left() {
    let quirky = Arc::new(Quirky::new(Quirk::ConflictSwaps(u32::MAX)));
    let d = DescriptorStore::new(quirky.clone(), BASE, DeleteMode::Tombstone).unwrap();
    let alias = AliasRecord::new("al", &["a", "b"]);
    let mut buf = [0u8; 4096];
    let len = alias.write(&mut buf).unwrap();
    let key = descriptor_key(BASE, "al").unwrap();
    assert!(matches!(
        quirky.inner.create_register(&key, &buf[..len]).unwrap(),
        SwapOutcome::Applied(_)
    ));
    let mut m = mutations(&d, &at_now);
    let (left, sent) = counted(quirky.meter(), || {
        m.detach_from_aliases("a", &names(&["al"])).unwrap()
    });
    assert_eq!(left, 1);
    assert_eq!(
        sent,
        [(Get, 4)],
        "four reads, each swap refused, then given up"
    );
}

#[test]
fn a_hinted_addition_is_swapped_again_to_fence_a_removal() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    index(&d, "a");
    index(&d, "b");
    let mut m = mutations(&d, &at_now);
    let before = AliasRecord::new("al", &["a"]);
    let g = m.create_alias(&before).unwrap();
    // `b` keeps a hint from an earlier membership that a removal, having
    // read `b`, is about to drop.
    let mut buf = [0u8; 4096];
    let mut read = [0u8; 4096];
    let current = d.resolve_raw("b", &mut read).unwrap();
    let Target::Index(mut record) = current.target else {
        panic!("b is an index");
    };
    record.aliased_by.push("al".into());
    let stale = d
        .update(&record, current.generation, &mut buf)
        .unwrap()
        .unwrap();
    m.update_alias(&before, &AliasRecord::new("al", &["a", "b"]), g)
        .unwrap()
        .unwrap();
    // The preparation swapped `b` though its hint was there, so the
    // removal's swap from what it read loses.
    record.aliased_by.clear();
    assert_eq!(d.update(&record, stale, &mut buf).unwrap(), None);
    assert_eq!(hints(&d, "b"), ["al"]);
}
