//! docs/specs/name-listing/SPEC.md: listing names, a bounded page at a time.

use oss_blob::S3Op::{Get, List};
use oss_blob::{SliceSource, SwapOutcome};
use oss_format::IndexRecord;
use oss_meta::{DeleteMode, DescriptorStore, MetaError, NameRule};

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_format::{MAX_TOMBSTONE_BYTES, Tombstone};

use crate::probe::{Quirk, Quirky};
use crate::{BASE, Backend, all, counted};

const NOW: i64 = 1_763_164_800_000;

fn create(d: &DescriptorStore, name: &str) -> u64 {
    let mut buf = vec![0u8; 4096];
    d.create(&IndexRecord::new(name, "u", 1), NOW, &mut buf)
        .unwrap()
}

/// Creates `name` and deletes it, leaving a tombstone.
fn tombstone(d: &DescriptorStore, name: &str) {
    let g = create(d, name);
    assert!(d.delete_if_unchanged(name, g, NOW).unwrap());
}

fn alias(b: &Backend, name: &str) {
    let value = format!(r#"{{"alias":"{name}","indices":["a"]}}"#);
    assert!(matches!(
        b.store
            .create_register(&format!("{BASE}indices/{name}"), value.as_bytes())
            .unwrap(),
        SwapOutcome::Applied(_)
    ));
}

/// One page's names and where it resumes.
fn page(
    d: &DescriptorStore,
    prefix: &str,
    after: Option<&str>,
    limit: usize,
) -> (Vec<String>, Option<String>) {
    let mut names = Vec::new();
    let mut buf = vec![0u8; 4096];
    let next = d
        .list_page(prefix, after, limit, &mut buf, &mut |r| {
            names.push(r.name.to_string());
        })
        .unwrap();
    (names, next)
}

/// Every page of a walk from the start.
fn walk(d: &DescriptorStore, prefix: &str, limit: usize) -> Vec<String> {
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let (names, next) = page(d, prefix, after.as_deref(), limit);
        assert!(names.len() <= limit);
        all.extend(names);
        match next {
            Some(n) => after = Some(n),
            None => return all,
        }
    }
}

/// Indexes `b`, `d`, `f`, `g`; tombstones `a`, `e`; an alias `c`.
fn mixed(b: &Backend) -> DescriptorStore {
    let d = b.descriptors(DeleteMode::Tombstone);
    tombstone(&d, "a");
    create(&d, "b");
    alias(b, "c");
    create(&d, "d");
    tombstone(&d, "e");
    create(&d, "f");
    create(&d, "g");
    d
}

#[test]
fn every_index_is_walked_once_in_name_order() {
    for b in all() {
        let d = mixed(&b);
        for limit in 1..=9 {
            assert_eq!(
                walk(&d, "", limit),
                ["b", "d", "f", "g"],
                "{:?} {limit}",
                b.kind
            );
        }
    }
}

#[test]
fn an_alias_is_skipped_without_being_decoded() {
    for b in all() {
        let d = mixed(&b);
        // An alias record no reader accepts, as the Java's listing skips it
        // by its discriminator alone.
        b.store
            .create_register(
                &format!("{BASE}indices/c2"),
                br#"{"alias":{"indices":["a"]}}"#,
            )
            .unwrap();
        assert_eq!(walk(&d, "", 3), ["b", "d", "f", "g"], "{:?}", b.kind);
        let mut buf = vec![0u8; 4096];
        assert!(d.resolve("c2", &mut buf).is_err());
    }
}

#[test]
fn a_page_honours_its_prefix_and_cursor() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        for name in ["logs-1", "logs-2", "logs2", "metrics-1"] {
            create(&d, name);
        }
        assert_eq!(
            page(&d, "logs-", None, 10),
            (vec!["logs-1".into(), "logs-2".into()], None)
        );
        assert_eq!(
            page(&d, "logs-", Some("logs-1"), 10),
            (vec!["logs-2".into()], None)
        );
        assert_eq!(
            page(&d, "", Some("logs2"), 10),
            (vec!["metrics-1".into()], None)
        );
        assert_eq!(page(&d, "x", None, 10), (vec![], None), "{:?}", b.kind);
    }
}

#[test]
fn a_page_costs_one_list_and_a_read_a_name() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        for name in ["a", "b", "c"] {
            create(&d, name);
        }
        let (out, sent) = counted(b.meter(), || page(&d, "", None, 10));
        assert_eq!(out, (vec!["a".into(), "b".into(), "c".into()], None));
        assert_eq!(sent, [(Get, 3), (List, 1)], "{:?}", b.kind);
        // Filled with names left: the listing says so, and no more is listed.
        let (out, sent) = counted(b.meter(), || page(&d, "", None, 2));
        assert_eq!(out, (vec!["a".into(), "b".into()], Some("b".into())));
        assert_eq!(sent, [(Get, 2), (List, 1)], "{:?}", b.kind);
        // Filled exactly at the end of the names: done, in the same LIST.
        let (out, sent) = counted(b.meter(), || page(&d, "", Some("a"), 2));
        assert_eq!(out, (vec!["b".into(), "c".into()], None));
        assert_eq!(sent, [(Get, 2), (List, 1)], "{:?}", b.kind);
    }
}

#[test]
fn tombstones_cost_reads_not_lists() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        create(&d, "a");
        tombstone(&d, "b");
        tombstone(&d, "c");
        create(&d, "d");
        create(&d, "e");
        let (out, sent) = counted(b.meter(), || page(&d, "", None, 2));
        // One LIST of all five; a to d read, and the page is full with e left.
        assert_eq!(out, (vec!["a".into(), "d".into()], Some("d".into())));
        assert_eq!(sent, [(Get, 4), (List, 1)], "{:?}", b.kind);
        // A page of one past both tombstones is still one LIST.
        let (out, sent) = counted(b.meter(), || page(&d, "", Some("a"), 1));
        assert_eq!(out, (vec!["d".into()], Some("d".into())));
        assert_eq!(sent, [(Get, 3), (List, 1)], "{:?}", b.kind);
        let (out, sent) = counted(b.meter(), || page(&d, "", Some("d"), 1));
        assert_eq!(out, (vec!["e".into()], None));
        assert_eq!(sent, [(Get, 1), (List, 1)], "{:?}", b.kind);
    }
}

#[test]
fn a_thousand_names_are_one_list() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 256];
        for i in 0..1001 {
            let name = format!("i{i:04}");
            let len = IndexRecord::new(&name, "u", 1).write(&mut buf).unwrap();
            b.store
                .create_register(&format!("{BASE}indices/{name}"), &buf[..len])
                .unwrap();
        }
        let (out, sent) = counted(b.meter(), || page(&d, "", None, 1000));
        assert_eq!(out.0.len(), 1000);
        assert_eq!(out.1.as_deref(), Some("i0999"));
        assert_eq!(sent, [(Get, 1000), (List, 1)], "{:?}", b.kind);
        let (out, sent) = counted(b.meter(), || page(&d, "", Some("i0999"), 1000));
        assert_eq!(out, (vec!["i1000".into()], None));
        assert_eq!(sent, [(Get, 1), (List, 1)], "{:?}", b.kind);
        // More than a LIST's page: two LISTs, nothing read.
        let (names, sent) = counted(b.meter(), || d.names_with_prefix("", 1001).unwrap());
        assert_eq!(names.len(), 1001);
        assert_eq!(sent, [(List, 2)], "{:?}", b.kind);
    }
}

#[test]
fn a_page_short_of_a_full_list_lists_on_from_the_last_key() {
    // Over a thousand tombstones, the page lists past them in pages.
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let d = DescriptorStore::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        BASE,
        DeleteMode::Tombstone,
    )
    .unwrap();
    let mut tombstone = [0u8; MAX_TOMBSTONE_BYTES];
    let len = Tombstone {
        deleted_at: NOW,
        reaping_at: 0,
    }
    .write(&mut tombstone)
    .unwrap();
    for i in 0..1000 {
        store
            .create_register(&format!("{BASE}indices/t{i:04}"), &tombstone[..len])
            .unwrap();
    }
    create(&d, "z");
    let (out, sent) = counted(store.meter(), || page(&d, "", None, 1));
    assert_eq!(out, (vec!["z".into()], None));
    assert_eq!(sent, [(Get, 1001), (List, 2)]);
}

#[test]
fn a_tombstone_is_recognised_not_parsed() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        b.store
            .create_register(
                &format!("{BASE}indices/a"),
                br#"{"tombstone":true,"deleted_at":"x"}"#,
            )
            .unwrap();
        create(&d, "b");
        assert_eq!(
            page(&d, "", None, 10),
            (vec!["b".into()], None),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_register_longer_than_the_buffer_fails_the_page() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        create(&d, "a");
        let got = d.list_page("", None, 10, &mut [0u8; 8], &mut |_| {});
        assert!(
            matches!(got, Err(MetaError::Blob(_))),
            "{:?} {got:?}",
            b.kind
        );
    }
}

#[test]
fn a_malformed_register_fails_the_page() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        create(&d, "a");
        b.store
            .create_register(&format!("{BASE}indices/b"), b"not json")
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let got = d.list_page("", None, 10, &mut buf, &mut |_| {});
        assert!(
            matches!(got, Err(MetaError::Format(_))),
            "{:?} {got:?}",
            b.kind
        );
    }
}

#[test]
fn a_key_below_a_name_is_not_a_name() {
    // Only object stores hold a key below another; a filesystem cannot.
    for b in all().into_iter().filter(|b| b.kind != crate::Kind::Fs) {
        let d = b.descriptors(DeleteMode::Tombstone);
        create(&d, "a");
        b.store
            .put(
                &format!("{BASE}indices/a/x"),
                &mut SliceSource::new(b"x"),
                &mut [0u8; 16],
            )
            .unwrap();
        create(&d, "b");
        let (out, sent) = counted(b.meter(), || page(&d, "", None, 10));
        assert_eq!(out, (vec!["a".into(), "b".into()], None));
        assert_eq!(sent, [(Get, 2), (List, 1)], "{:?}", b.kind);
        let (names, _) = counted(b.meter(), || d.names_with_prefix("", 10).unwrap());
        assert_eq!(names, ["a", "b"]);
        // The nested key is not counted against the cap.
        let (names, sent) = counted(b.meter(), || d.names_with_prefix("", 2).unwrap());
        assert_eq!(names, ["a", "b"]);
        assert_eq!(sent, [(List, 2)], "{:?}", b.kind);
        // Nor read once the cap is passed.
        let (got, sent) = counted(b.meter(), || d.names_with_prefix("", 1));
        assert!(matches!(got, Err(MetaError::TooManyMatches { cap: 1, .. })));
        assert_eq!(sent, [(Get, 2), (List, 2)], "{:?}", b.kind);
    }
}

/// `logs-a` (with a mapping too long for a tombstone's buffer) and
/// `logs-b`, indexes; `logs-c`, a tombstone; `logs-d`, an alias; `other`.
fn logs(b: &Backend) -> DescriptorStore {
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut record = IndexRecord::new("logs-a", "u", 1);
    let mapping = format!(
        r#"{{"properties":{{"f":{{"type":"keyword","x":"{}"}}}}}}"#,
        "y".repeat(200)
    );
    record.mapping = Some(mapping.into());
    d.create(&record, NOW, &mut vec![0u8; 4096]).unwrap();
    create(&d, "logs-b");
    tombstone(&d, "logs-c");
    alias(b, "logs-d");
    create(&d, "other");
    d
}

#[test]
fn names_within_the_cap_are_listed_unread() {
    for b in all() {
        let d = logs(&b);
        let (names, sent) = counted(b.meter(), || d.names_with_prefix("logs-", 4).unwrap());
        // Unread, so the tombstone is answered too: the caller reads the names.
        assert_eq!(names, ["logs-a", "logs-b", "logs-c", "logs-d"]);
        assert_eq!(sent, [(List, 1)], "{:?}", b.kind);
    }
}

#[test]
fn past_the_cap_only_live_names_count() {
    for b in all() {
        let d = logs(&b);
        let (names, sent) = counted(b.meter(), || d.names_with_prefix("logs-", 3).unwrap());
        assert_eq!(names, ["logs-a", "logs-b", "logs-d"]);
        // The three listed are read, then the rest listed and read.
        assert_eq!(sent, [(Get, 4), (List, 2)], "{:?}", b.kind);
    }
}

#[test]
fn more_live_names_than_the_cap_are_refused() {
    for b in all() {
        let d = logs(&b);
        let (got, sent) = counted(b.meter(), || d.names_with_prefix("logs-", 2));
        let Err(MetaError::TooManyMatches { prefix, cap }) = &got else {
            panic!("{:?} {got:?}", b.kind);
        };
        assert_eq!((prefix.as_str(), *cap), ("logs-", 2));
        assert_eq!(
            got.unwrap_err().to_string(),
            "more than 2 indices begin with [logs-]. Narrowing the pattern or naming the \
             indices is the answer; returning the first 2 would be a partial result that \
             looks complete."
        );
        // Stopped at the third live name: other is never read.
        assert_eq!(sent, [(Get, 4), (List, 2)], "{:?}", b.kind);
        let (got, sent) = counted(b.meter(), || d.names_with_prefix("", 1));
        assert!(matches!(got, Err(MetaError::TooManyMatches { cap: 1, .. })));
        assert_eq!(sent, [(Get, 2), (List, 2)], "{:?}", b.kind);
    }
}

#[test]
fn a_bad_prefix_or_bound_sends_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 64];
        let (got, sent) = counted(b.meter(), || {
            [
                d.list_page("a*", None, 1, &mut buf, &mut |_| {})
                    .map(|_| ()),
                d.list_page("a", None, 0, &mut buf, &mut |_| {}).map(|_| ()),
                d.names_with_prefix("a:", 1).map(|_| ()),
                d.names_with_prefix("a", 0).map(|_| ()),
            ]
        });
        assert!(sent.is_empty(), "{:?} {sent:?}", b.kind);
        let [star, limit, colon, cap] = got;
        assert!(
            matches!(star, Err(MetaError::InvalidName(e)) if e.rule == NameRule::PatternChar('*'))
        );
        assert!(matches!(limit, Err(MetaError::InvalidArgument(_))));
        assert!(
            matches!(colon, Err(MetaError::InvalidName(e)) if e.rule == NameRule::PatternChar(':'))
        );
        assert!(matches!(cap, Err(MetaError::InvalidArgument(_))));
    }
}

fn quirky(quirk: Quirk, names: &[&str]) -> DescriptorStore {
    let d =
        DescriptorStore::new(Arc::new(Quirky::new(quirk)), BASE, DeleteMode::Tombstone).unwrap();
    for name in names {
        create(&d, name);
    }
    d
}

#[test]
fn a_name_deleted_since_it_was_listed_is_skipped() {
    let d = quirky(Quirk::PhantomKey, &["a", "b", "c"]);
    let mut names = Vec::new();
    let next = d
        .list_page("", None, 10, &mut vec![0u8; 4096], &mut |r| {
            names.push(r.name.to_string())
        })
        .unwrap();
    assert_eq!(
        (names, next),
        (vec!["a".to_owned(), "b".into(), "c".into()], None)
    );
    // Past the cap a gone name is not live: a and b are, then c passes.
    assert!(matches!(
        d.names_with_prefix("", 2),
        Err(MetaError::TooManyMatches { cap: 2, .. })
    ));
}

#[test]
fn an_empty_page_claiming_more_ends_the_walk() {
    let mut buf = vec![0u8; 4096];
    let d = quirky(Quirk::EmptyTruncatedFrom(1), &["a"]);
    assert_eq!(
        d.list_page("", None, 1, &mut buf, &mut |_| panic!())
            .unwrap(),
        None
    );
    assert!(d.names_with_prefix("", 1).unwrap().is_empty());
    // The first LIST passes the cap; the next comes back empty.
    let d = quirky(Quirk::EmptyTruncatedFrom(2), &["a", "b"]);
    assert_eq!(d.names_with_prefix("", 1).unwrap(), ["a"]);
}

#[test]
fn a_failed_read_past_the_cap_fails_the_lookup() {
    // Two creates, the LIST, then the read of the first name.
    let d = quirky(Quirk::FailRequest(4), &["a", "b"]);
    assert!(matches!(
        d.names_with_prefix("", 1),
        Err(MetaError::Blob(_))
    ));
}

#[test]
fn past_the_cap_the_walk_reads_on_a_page_at_a_time() {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let d = DescriptorStore::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        BASE,
        DeleteMode::Tombstone,
    )
    .unwrap();
    create(&d, "a");
    let mut tombstone = [0u8; MAX_TOMBSTONE_BYTES];
    let len = Tombstone {
        deleted_at: NOW,
        reaping_at: 0,
    }
    .write(&mut tombstone)
    .unwrap();
    for i in 0..1000 {
        store
            .create_register(&format!("{BASE}indices/t{i:04}"), &tombstone[..len])
            .unwrap();
    }
    create(&d, "z");
    let (got, sent) = counted(store.meter(), || d.names_with_prefix("", 1));
    assert!(matches!(got, Err(MetaError::TooManyMatches { cap: 1, .. })));
    // a, then a page of a thousand tombstones, then z: the second live name.
    assert_eq!(sent, [(Get, 1002), (List, 3)]);
    let (names, sent) = counted(store.meter(), || d.names_with_prefix("", 2).unwrap());
    assert_eq!(names, ["a", "z"]);
    // a and t0000 within the cap, then the other 1,000 keys in one page.
    assert_eq!(sent, [(Get, 1002), (List, 2)]);
}
