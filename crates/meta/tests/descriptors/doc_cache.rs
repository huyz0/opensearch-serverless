//! docs/specs/doc-set-cache/SPEC.md: a node's snapshot of a document set.

use std::sync::Arc;

use oss_blob::S3Op::Get;
use oss_meta::{DocSet, DocSetCache, MetaError};

use crate::doc_sets::{body, healing, marker, put, set};
use crate::{all, counted};

fn snap(s: &DocSet, c: &DocSetCache) -> Arc<oss_meta::DocSnapshot> {
    s.snapshot(c, &mut marker(), &mut body()).unwrap()
}

fn texts(snap: &oss_meta::DocSnapshot) -> Vec<(String, String)> {
    snap.docs()
        .map(|(n, d)| (n.to_owned(), String::from_utf8(d.to_vec()).unwrap()))
        .collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_owned(), b.to_owned())
}

#[test]
fn a_snapshot_reads_the_marker_and_only_the_documents_that_moved() {
    for b in all() {
        let s = set(&b);
        let cache = DocSetCache::new();
        for (n, t) in [("a", "1"), ("b", "2"), ("c", "3")] {
            put(&s, n, t);
        }
        let (first, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 4)], "first {:?}", b.kind);
        assert_eq!(first.version(), 3);
        assert_eq!(
            texts(&first),
            [pair("a", "1"), pair("b", "2"), pair("c", "3")]
        );
        // Unchanged: the marker alone, and the same snapshot back.
        let (again, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 1)], "unchanged {:?}", b.kind);
        assert!(Arc::ptr_eq(&first, &again));
        // One document replaced: the marker and that document.
        put(&s, "b", "22");
        let (second, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 2)], "one moved {:?}", b.kind);
        assert_eq!(
            texts(&second),
            [pair("a", "1"), pair("b", "22"), pair("c", "3")]
        );
        // Unmoved documents are the same bytes, not copies.
        let bytes = |s: &oss_meta::DocSnapshot, n: &str| s.get(n).map(<[u8]>::as_ptr);
        assert_eq!(bytes(&first, "a"), bytes(&second, "a"));
        assert_ne!(bytes(&first, "b"), bytes(&second, "b"));
        // One deleted: the marker alone.
        assert!(s.delete("a", &mut marker()).unwrap());
        let (third, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 1)], "deleted {:?}", b.kind);
        assert_eq!(texts(&third), [pair("b", "22"), pair("c", "3")]);
        assert_eq!(third.get("a"), None);
        assert_eq!(third.get("b"), Some(&b"22"[..]));
    }
}

#[test]
fn an_empty_set_is_one_request_and_then_cached() {
    for b in all() {
        let s = set(&b);
        let cache = DocSetCache::new();
        let (empty, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!((empty.version(), empty.docs().count()), (0, 0));
        assert_eq!(sent, [(Get, 1)]);
        let (again, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 1)]);
        assert!(Arc::ptr_eq(&empty, &again));
    }
}

#[test]
fn a_replaced_document_is_not_taken_for_the_cached_one() {
    for b in all() {
        let s = set(&b);
        let cache = DocSetCache::new();
        put(&s, "a", "1");
        let old = snap(&s, &cache);
        // Same names, other bytes, under a version the cache has seen: the
        // entries differ, so the snapshot is rebuilt.
        put(&s, "a", "2");
        assert_eq!(old.version() + 1, s.version(&mut marker()).unwrap());
        assert_eq!(texts(&snap(&s, &cache)), [pair("a", "2")]);
    }
}

#[test]
fn a_set_over_the_budget_is_returned_but_not_kept() {
    for b in all() {
        let s = set(&b);
        let cache = DocSetCache::with_budget(100);
        put(&s, "a", "1");
        let (first, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 2)]);
        assert_eq!(texts(&first), [pair("a", "1")]);
        let (_, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 2)], "not kept {:?}", b.kind);
    }
}

#[test]
fn a_set_just_under_the_budget_is_kept() {
    let b = &all()[0];
    let s = set(b);
    put(&s, "a", "1");
    let probe = DocSetCache::with_budget(usize::MAX);
    let kept = snap(&s, &probe).retained_bytes();
    let cache = DocSetCache::with_budget(kept);
    snap(&s, &cache);
    let (_, sent) = counted(b.meter(), || snap(&s, &cache));
    assert_eq!(sent, [(Get, 1)]);
    let tight = DocSetCache::with_budget(kept - 1);
    snap(&s, &tight);
    let (_, sent) = counted(b.meter(), || snap(&s, &tight));
    assert_eq!(sent, [(Get, 2)]);
}

#[test]
fn a_document_dropped_mid_read_restarts_from_the_marker() {
    let (store, s) = healing();
    let cache = DocSetCache::new();
    let got = s.snapshot(&cache, &mut marker(), &mut body()).unwrap();
    assert_eq!(texts(&got), [pair("a", "fixed")]);
    drop(store);
}

#[test]
fn a_document_the_store_lost_is_reported() {
    for b in all() {
        b.store
            .create_register(
                &format!("{}templates/index/_version", crate::BASE),
                br#"{"version":1,"entries":{"a":"gone"}}"#,
            )
            .unwrap();
        let s = set(&b);
        let err = s
            .snapshot(&DocSetCache::new(), &mut marker(), &mut body())
            .unwrap_err();
        assert!(matches!(err, MetaError::MissingContent(_)), "{err}");
    }
}

#[test]
fn a_changed_set_costs_what_changed_not_what_is_held() {
    for b in all() {
        let s = set(&b);
        let cache = DocSetCache::new();
        for i in 0..20 {
            put(&s, &format!("t{i:02}"), "x");
        }
        snap(&s, &cache);
        put(&s, "t07", "y");
        let (_, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!(sent, [(Get, 2)], "{:?}", b.kind);
    }
}

#[test]
fn a_set_recreated_at_the_same_version_is_not_taken_for_the_cached_one() {
    use oss_blob::SliceSource;
    for b in all() {
        let key = format!("{}templates/index/_version", crate::BASE);
        let blob = |id: &str, text: &[u8]| {
            b.store
                .put(
                    &format!("{}templates/index/blob-{id}", crate::BASE),
                    &mut SliceSource::new(text),
                    &mut [0u8; 64],
                )
                .unwrap();
        };
        blob("k1", b"old");
        blob("k2", b"new");
        b.store
            .create_register(&key, br#"{"version":1,"entries":{"a":"k1"}}"#)
            .unwrap();
        let s = set(&b);
        let cache = DocSetCache::new();
        assert_eq!(texts(&snap(&s, &cache)), [pair("a", "old")]);
        let generation = b
            .store
            .read_register(&key, &mut [0u8; 128])
            .unwrap()
            .unwrap()
            .generation;
        assert!(b.store.delete_register(&key, generation).unwrap());
        b.store
            .create_register(&key, br#"{"version":1,"entries":{"a":"k2"}}"#)
            .unwrap();
        assert_eq!(texts(&snap(&s, &cache)), [pair("a", "new")]);
        // Same content keys, other version: rebuilt from the held bytes.
        b.store
            .swap_register(
                &key,
                b.store
                    .read_register(&key, &mut [0u8; 128])
                    .unwrap()
                    .unwrap()
                    .generation,
                br#"{"version":2,"entries":{"a":"k2"}}"#,
            )
            .unwrap();
        let (got, sent) = counted(b.meter(), || snap(&s, &cache));
        assert_eq!((got.version(), sent), (2, vec![(Get, 1)]), "{:?}", b.kind);
    }
}
