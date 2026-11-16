//! docs/specs/routing-cache/SPEC.md: routing answers from a recent read,
//! and a node sees its own changes at once.

use std::sync::{Arc, Mutex};

use oss_blob::BlobStore;
use oss_blob::S3Op::Get;
use oss_blob::register::ABSENT_GENERATION;
use oss_format::{IndexRecord, descriptor_key};
use oss_meta::{
    AliasMutation, AliasRecord, DEFAULT_ROUTING_CACHE_MILLIS, DeleteMode, DescriptorStore,
    MetaError, RoutingCache, Target,
};

use crate::probe::Quirky;
use crate::{BASE, all, counted};

const NOW: i64 = 1_763_164_800_000;

/// A store whose changes this node's cache forgets.
fn wired(b: &crate::Backend, cache: &Arc<RoutingCache>) -> DescriptorStore {
    let forget = Arc::clone(cache);
    b.descriptors(DeleteMode::Tombstone)
        .on_changed(Arc::new(move |name: &str| forget.forget(name)))
}

fn uuid(cache: &RoutingCache, d: &DescriptorStore, name: &str, now: i64) -> Option<String> {
    let mut buf = [0u8; 4096];
    match cache.resolve(d, name, now, &mut buf).unwrap().target {
        Target::Index(record) => Some(record.uuid.to_string()),
        Target::Alias(_) | Target::Absent => None,
    }
}

#[test]
fn a_name_read_once_routes_without_a_request_for_the_window() {
    for b in all() {
        let cache = RoutingCache::new();
        let d = b.descriptors(DeleteMode::Tombstone);
        let g = d
            .create(&IndexRecord::new("logs", "u1", 1), NOW, &mut [0u8; 4096])
            .unwrap();
        let mut buf = [0u8; 4096];
        let (first, sent) = counted(b.meter(), || {
            cache.resolve(&d, "logs", NOW, &mut buf).unwrap().generation
        });
        assert_eq!((first, sent), (g, vec![(Get, 1)]), "{:?}", b.kind);
        let last = NOW + DEFAULT_ROUTING_CACHE_MILLIS;
        let (hit, sent) = counted(b.meter(), || uuid(&cache, &d, "logs", last));
        assert_eq!((hit.as_deref(), sent), (Some("u1"), vec![]), "{:?}", b.kind);
        let (_, sent) = counted(b.meter(), || uuid(&cache, &d, "logs", last + 1));
        assert_eq!(sent, [(Get, 1)], "past the window, {:?}", b.kind);
    }
}

#[test]
fn a_miss_is_never_cached() {
    for b in all() {
        let cache = RoutingCache::new();
        let d = b.descriptors(DeleteMode::Tombstone);
        assert_eq!(uuid(&cache, &d, "logs", NOW), None);
        let mut buf = [0u8; 4096];
        let resolved = cache.resolve(&d, "logs", NOW, &mut buf).unwrap();
        assert_eq!(resolved.generation, ABSENT_GENERATION);
        assert_eq!(cache.size(), (0, 0));
        // Created elsewhere, it routes at once.
        d.create(&IndexRecord::new("logs", "u1", 1), NOW, &mut [0u8; 4096])
            .unwrap();
        assert_eq!(
            uuid(&cache, &d, "logs", NOW).as_deref(),
            Some("u1"),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn this_nodes_own_changes_route_at_once() {
    for b in all() {
        let cache = Arc::new(RoutingCache::new());
        let d = wired(&b, &cache);
        let mut buf = [0u8; 4096];
        let g = d
            .create(&IndexRecord::new("logs", "u1", 1), NOW, &mut buf)
            .unwrap();
        assert_eq!(uuid(&cache, &d, "logs", NOW).as_deref(), Some("u1"));
        d.update(&IndexRecord::new("logs", "u2", 1), g, &mut buf)
            .unwrap()
            .unwrap();
        let (now, sent) = counted(b.meter(), || uuid(&cache, &d, "logs", NOW));
        assert_eq!(
            (now.as_deref(), sent),
            (Some("u2"), vec![(Get, 1)]),
            "{:?}",
            b.kind
        );
        d.delete("logs").unwrap();
        assert_eq!(uuid(&cache, &d, "logs", NOW), None);
        assert_eq!(cache.size(), (0, 0));
    }
}

#[test]
fn another_nodes_change_routes_within_the_window() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    let here = b.descriptors(DeleteMode::Tombstone);
    let there = b.descriptors(DeleteMode::Tombstone);
    let mut buf = [0u8; 4096];
    let g = here
        .create(&IndexRecord::new("logs", "u1", 1), NOW, &mut buf)
        .unwrap();
    assert_eq!(uuid(&cache, &here, "logs", NOW).as_deref(), Some("u1"));
    there
        .update(&IndexRecord::new("logs", "u2", 1), g, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(uuid(&cache, &here, "logs", NOW + 1).as_deref(), Some("u1"));
    let later = NOW + DEFAULT_ROUTING_CACHE_MILLIS + 1;
    assert_eq!(uuid(&cache, &here, "logs", later).as_deref(), Some("u2"));
}

#[test]
fn every_applied_change_is_told_and_no_other() {
    let b = &all()[0];
    let told = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&told);
    let d = b
        .descriptors(DeleteMode::Tombstone)
        .on_changed(Arc::new(move |name: &str| {
            into.lock().unwrap().push(name.to_owned())
        }));
    let mut buf = [0u8; 4096];
    let g = d
        .create(&IndexRecord::new("a", "u", 1), NOW, &mut buf)
        .unwrap();
    assert!(
        d.create(&IndexRecord::new("a", "u", 1), NOW, &mut buf)
            .is_err()
    );
    let g = d
        .update(&IndexRecord::new("a", "v", 1), g, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(
        d.update(&IndexRecord::new("a", "w", 1), g + 1, &mut buf)
            .unwrap(),
        None
    );
    let al = AliasRecord::new("al", &["a"]);
    let ga = d.create_alias(&al, NOW, &mut buf).unwrap();
    let ga = d.update_alias(&al, ga, &mut buf).unwrap().unwrap();
    assert_eq!(d.update_alias(&al, ga + 1, &mut buf).unwrap(), None);
    assert!(!d.delete_if_unchanged("al", ga + 1, NOW).unwrap());
    assert!(d.delete_if_unchanged("al", ga, NOW).unwrap());
    // A create over the alias's tombstone is a change too.
    d.create(&IndexRecord::new("al", "u", 1), NOW, &mut buf)
        .unwrap();
    d.delete("a").unwrap();
    assert_eq!(
        *told.lock().unwrap(),
        ["a", "a", "al", "al", "al", "al", "a"]
    );
}

#[test]
fn a_window_of_zero_reads_through_every_time() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    cache.set_window_millis(-5);
    let d = b.descriptors(DeleteMode::Tombstone);
    d.create(&IndexRecord::new("logs", "u1", 1), NOW, &mut [0u8; 4096])
        .unwrap();
    for _ in 0..2 {
        let (_, sent) = counted(b.meter(), || uuid(&cache, &d, "logs", NOW));
        assert_eq!(sent, [(Get, 1)]);
    }
    assert_eq!(cache.size(), (0, 0));
}

#[test]
fn the_cache_holds_its_byte_budget() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut buf = [0u8; 4096];
    d.create(&IndexRecord::new("small", "u", 1), NOW, &mut buf)
        .unwrap();
    let big = IndexRecord {
        mapping: Some(r#"{"properties":{"f":{"type":"keyword"}}}"#.repeat(20).into()),
        ..IndexRecord::new("big", "u", 1)
    };
    d.create(&big, NOW, &mut buf).unwrap();
    uuid(&cache, &d, "small", NOW);
    let (_, small) = cache.size();
    // Room for the small one twice over, not the big one.
    cache.set_budget_bytes(small * 2);
    uuid(&cache, &d, "big", NOW);
    assert_eq!(
        cache.size(),
        (1, small),
        "a register past the budget is not kept"
    );
    let (_, sent) = counted(b.meter(), || uuid(&cache, &d, "small", NOW));
    assert_eq!(sent, [], "the small one stays");
    // Expired entries make room.
    let later = NOW + DEFAULT_ROUTING_CACHE_MILLIS + 1;
    d.create(&IndexRecord::new("other", "u", 1), NOW, &mut buf)
        .unwrap();
    uuid(&cache, &d, "other", NOW);
    assert_eq!(cache.size().0, 2);
    d.create(&IndexRecord::new("third", "u", 1), NOW, &mut buf)
        .unwrap();
    uuid(&cache, &d, "third", later);
    assert_eq!(
        cache.size().0,
        1,
        "the expired two are dropped for the third"
    );
    // A smaller budget drops what no longer fits.
    cache.set_budget_bytes(small - 1);
    assert_eq!(cache.size(), (0, 0));
    cache.set_budget_bytes(small);
    assert_eq!(cache.size(), (0, 0));
}

#[test]
fn past_the_threshold_expired_names_are_dropped_once_a_window() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    cache.set_prune_above(1);
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut buf = [0u8; 4096];
    for name in ["a", "b", "c", "d"] {
        d.create(&IndexRecord::new(name, "u", 1), NOW, &mut buf)
            .unwrap();
    }
    uuid(&cache, &d, "a", NOW);
    uuid(&cache, &d, "b", NOW);
    // Pruned at NOW, with nothing yet expired.
    assert_eq!(cache.size().0, 2);
    let later = NOW + DEFAULT_ROUTING_CACHE_MILLIS + 1;
    uuid(&cache, &d, "c", later);
    assert_eq!(cache.size().0, 1, "`a` and `b` expired and were dropped");
    // Not again within the window.
    let then = later + 1 + DEFAULT_ROUTING_CACHE_MILLIS + 1;
    uuid(&cache, &d, "d", later + 1);
    assert_eq!(cache.size().0, 2);
    uuid(&cache, &d, "a", then);
    assert_eq!(cache.size().0, 1);
}

#[test]
fn a_read_racing_a_local_change_is_not_kept() {
    let slot: Arc<Mutex<Option<Arc<RoutingCache>>>> = Arc::new(Mutex::new(None));
    let racer = Arc::clone(&slot);
    // This node changes `logs` while the cache's read is in flight.
    let quirky = Arc::new(Quirky::hooked(move |_: &dyn BlobStore| {
        if let Some(cache) = &*racer.lock().unwrap() {
            cache.forget("logs");
        }
    }));
    let d = DescriptorStore::new(quirky.clone(), BASE, DeleteMode::Tombstone).unwrap();
    let cache = Arc::new(RoutingCache::new());
    *slot.lock().unwrap() = Some(Arc::clone(&cache));
    d.create(&IndexRecord::new("logs", "u1", 1), NOW, &mut [0u8; 4096])
        .unwrap();
    quirky.arm(1);
    assert_eq!(uuid(&cache, &d, "logs", NOW).as_deref(), Some("u1"));
    assert_eq!(cache.size(), (0, 0), "the read began before the change");
    assert_eq!(uuid(&cache, &d, "logs", NOW).as_deref(), Some("u1"));
    assert_eq!(cache.size().0, 1);
    // A racing change also stops an absent read from forgetting: there is
    // nothing to forget, and the change already did.
    quirky.arm(1);
    assert_eq!(uuid(&cache, &d, "none", NOW), None);
}

#[test]
fn an_alias_being_created_routes_as_absent_and_is_not_kept() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut buf = [0u8; 4096];
    let pending = AliasRecord {
        mutation: Some(AliasMutation {
            until: NOW,
            creating: true,
            additions: Vec::new(),
        }),
        ..AliasRecord::new("al", &["a"])
    };
    let g = d.create_alias(&pending, NOW, &mut buf).unwrap();
    let resolved = cache.resolve(&d, "al", NOW, &mut buf).unwrap();
    assert_eq!((resolved.generation, resolved.target), (g, Target::Absent));
    assert_eq!(cache.size(), (0, 0));
    // A published alias is kept, and answers as itself.
    d.update_alias(&AliasRecord::new("al", &["a"]), g, &mut buf)
        .unwrap()
        .unwrap();
    cache.resolve(&d, "al", NOW, &mut buf).unwrap();
    let (hit, sent) = counted(b.meter(), || {
        matches!(
            cache
                .resolve(&d, "al", NOW, &mut [0u8; 4096])
                .unwrap()
                .target,
            Target::Alias(_)
        )
    });
    assert!(hit);
    assert_eq!(sent, []);
}

#[test]
fn a_malformed_or_oversized_register_is_refused_not_kept() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    let d = b.descriptors(DeleteMode::Tombstone);
    let key = descriptor_key(BASE, "bad").unwrap();
    b.store.create_register(&key, b"{\"name\":7}").unwrap();
    let got = cache.resolve(&d, "bad", NOW, &mut [0u8; 4096]).map(|_| ());
    assert!(matches!(got, Err(MetaError::Format(_))), "{got:?}");
    assert_eq!(cache.size(), (0, 0));
    // A cached register longer than the caller's buffer is read through,
    // and refused as the store refuses it.
    d.create(&IndexRecord::new("logs", "u1", 1), NOW, &mut [0u8; 4096])
        .unwrap();
    uuid(&cache, &d, "logs", NOW);
    let (got, sent) = counted(b.meter(), || {
        cache.resolve(&d, "logs", NOW, &mut [0u8; 8]).map(|_| ())
    });
    assert!(matches!(got, Err(MetaError::Blob(_))), "{got:?}");
    assert_eq!(sent, [(Get, 1)]);
    let got = cache.resolve(&d, "Bad", NOW, &mut [0u8; 8]).map(|_| ());
    assert!(matches!(got, Err(MetaError::InvalidName(_))));
}

#[test]
fn a_default_cache_is_a_new_one() {
    let cache = RoutingCache::default();
    assert_eq!(cache.size(), (0, 0));
}

#[test]
fn expired_names_stay_until_they_are_in_the_way() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut buf = [0u8; 4096];
    for name in ["aa", "bb", "cc"] {
        d.create(&IndexRecord::new(name, "u", 1), NOW, &mut buf)
            .unwrap();
    }
    uuid(&cache, &d, "aa", NOW);
    let (_, one) = cache.size();
    // Below the name threshold, and within the budget exactly, nothing
    // expired is walked or dropped: it answers nothing, and costs no walk.
    cache.set_budget_bytes(one * 2);
    let later = NOW + DEFAULT_ROUTING_CACHE_MILLIS + 1;
    uuid(&cache, &d, "bb", later);
    assert_eq!(cache.size(), (2, one * 2));
    // A budget the cache holds exactly keeps it all.
    cache.set_budget_bytes(one * 2);
    assert_eq!(cache.size().0, 2);
    // Only when a read does not fit are the expired dropped for it.
    uuid(&cache, &d, "cc", later);
    assert_eq!(cache.size(), (2, one * 2));
}

#[test]
fn a_prune_comes_a_full_window_after_the_last() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    cache.set_prune_above(1);
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut buf = [0u8; 4096];
    for name in ["a", "b", "c"] {
        d.create(&IndexRecord::new(name, "u", 1), NOW, &mut buf)
            .unwrap();
    }
    uuid(&cache, &d, "a", NOW - 1);
    // The first prune, at NOW, finds nothing expired.
    uuid(&cache, &d, "b", NOW);
    // A window later exactly, the next may run: `a` has expired, `b` not.
    uuid(&cache, &d, "c", NOW + DEFAULT_ROUTING_CACHE_MILLIS);
    assert_eq!(cache.size().0, 2);
}

#[test]
fn a_name_found_gone_is_forgotten() {
    let b = &all()[0];
    let cache = RoutingCache::new();
    let here = b.descriptors(DeleteMode::Tombstone);
    let there = b.descriptors(DeleteMode::Tombstone);
    here.create(&IndexRecord::new("logs", "u1", 1), NOW, &mut [0u8; 4096])
        .unwrap();
    uuid(&cache, &here, "logs", NOW);
    there.delete("logs").unwrap();
    let later = NOW + DEFAULT_ROUTING_CACHE_MILLIS + 1;
    assert_eq!(uuid(&cache, &here, "logs", later), None);
    assert_eq!(cache.size(), (0, 0));
}
