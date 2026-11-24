//! docs/specs/doc-sets/SPEC.md, AC-3 to AC-6: document sets over the three
//! backends, with exact request counts.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_blob::S3Op::{Delete, Get, GetRange, Put};
use oss_blob::SliceSource;
use oss_meta::{DocKind, DocSet, MAX_DOC_BYTES, MAX_DOCS, MetaError};

use crate::probe::{Quirk, Quirky};
use crate::{BASE, Backend, all, counted};

const KIND: DocKind = DocKind::IndexTemplate;

pub(crate) fn set(b: &Backend) -> DocSet {
    DocSet::new(Arc::clone(&b.store), BASE, KIND).unwrap()
}

pub(crate) fn marker() -> Vec<u8> {
    vec![0u8; oss_meta::MAX_DOC_SET_MARKER_BYTES]
}

pub(crate) fn body() -> Vec<u8> {
    vec![0u8; 4096]
}

fn read(s: &DocSet, name: &str) -> Option<String> {
    let mut b = body();
    s.get(name, &mut marker(), &mut b)
        .unwrap()
        .map(|n| String::from_utf8(b[..n].to_vec()).unwrap())
}

pub(crate) fn put(s: &DocSet, name: &str, text: &str) {
    s.put(name, text.as_bytes(), false, &mut marker()).unwrap();
}

#[test]
fn a_document_is_put_read_replaced_and_deleted() {
    for b in all() {
        let s = set(&b);
        assert_eq!(read(&s, "logs"), None);
        put(&s, "logs", r#"{"index_patterns":["logs-*"]}"#);
        assert_eq!(
            read(&s, "logs").as_deref(),
            Some(r#"{"index_patterns":["logs-*"]}"#)
        );
        put(&s, "logs", r#"{"index_patterns":["l-*"]}"#);
        assert_eq!(
            read(&s, "logs").as_deref(),
            Some(r#"{"index_patterns":["l-*"]}"#)
        );
        assert_eq!(s.version(&mut marker()).unwrap(), 2);
        assert!(s.delete("logs", &mut marker()).unwrap());
        assert!(!s.delete("logs", &mut marker()).unwrap());
        assert_eq!(read(&s, "logs"), None);
        assert_eq!(s.version(&mut marker()).unwrap(), 3, "{:?}", b.kind);
    }
}

#[test]
fn the_javas_marker_and_content_read_as_a_set() {
    for b in all() {
        let mut m = marker();
        let java = r#"{"version":3,"entries":{"a":"h-1","b":"h-1"}}"#;
        b.store
            .create_register(&format!("{BASE}templates/index/_version"), java.as_bytes())
            .unwrap();
        b.store
            .put(
                &format!("{BASE}templates/index/blob-h-1"),
                &mut SliceSource::new(b"{}"),
                &mut [0u8; 64],
            )
            .unwrap();
        let s = set(&b);
        assert_eq!(s.names(&mut m).unwrap(), ["a", "b"]);
        assert_eq!(read(&s, "b").as_deref(), Some("{}"));
        // `a` and `b` share one blob: deleting `a` must leave it for `b`.
        assert!(s.delete("a", &mut m).unwrap());
        assert_eq!(read(&s, "b").as_deref(), Some("{}"), "{:?}", b.kind);
    }
}

#[test]
fn require_absent_refuses_an_existing_name_and_keeps_the_document() {
    for b in all() {
        let s = set(&b);
        put(&s, "t", "1");
        let err = s.put("t", b"2", true, &mut marker()).unwrap_err();
        assert!(matches!(err, MetaError::AlreadyExists));
        assert_eq!(read(&s, "t").as_deref(), Some("1"));
        s.put("u", b"2", true, &mut marker()).unwrap();
    }
}

#[test]
fn a_full_set_refuses_a_new_name_and_still_replaces() {
    let b = &all()[0];
    let s = set(b);
    for i in 0..MAX_DOCS {
        s.put(&format!("t{i}"), b"x", false, &mut marker()).unwrap();
    }
    let err = s.put("one-more", b"x", false, &mut marker()).unwrap_err();
    assert!(matches!(err, MetaError::TooManyDocs));
    s.put("t0", b"y", false, &mut marker()).unwrap();
    assert_eq!(read(&s, "t0").as_deref(), Some("y"));
    assert_eq!(s.names(&mut marker()).unwrap().len(), MAX_DOCS);
}

#[test]
fn a_bad_name_or_document_sends_nothing() {
    for b in all() {
        let s = set(&b);
        let ((), sent) = counted(b.meter(), || {
            for (name, text) in [
                ("", "must not be empty"),
                ("a/b", "must not contain '/'"),
                ("_a", "must not start with '_'"),
                ("..", "must not be '.' or '..'"),
                ("a\u{7}", "must not contain control characters"),
            ] {
                let err = s.put(name, b"x", false, &mut marker()).unwrap_err();
                assert!(err.to_string().ends_with(text), "{err}");
                assert!(s.get(name, &mut marker(), &mut body()).is_err());
                assert!(s.delete(name, &mut marker()).is_err());
            }
            let big = vec![b'x'; MAX_DOC_BYTES + 1];
            assert!(s.put("t", &big, false, &mut marker()).is_err());
        });
        assert!(sent.is_empty(), "{sent:?}");
    }
}

#[test]
fn a_document_the_marker_names_but_the_store_lost_is_reported() {
    for b in all() {
        b.store
            .create_register(
                &format!("{BASE}templates/index/_version"),
                br#"{"version":1,"entries":{"a":"gone"}}"#,
            )
            .unwrap();
        let s = set(&b);
        let err = s.get("a", &mut marker(), &mut body()).unwrap_err();
        assert!(matches!(err, MetaError::MissingContent(_)), "{err}");
        let err = s
            .for_each(&mut marker(), &mut body(), &mut |_, _| {})
            .unwrap_err();
        assert!(matches!(err, MetaError::MissingContent(_)));
    }
}

#[test]
fn request_counts_per_operation() {
    for b in all() {
        let s = set(&b);
        // A swap on the filesystem store also reads the register's length.
        let swap_extra: Vec<(oss_blob::S3Op, u64)> = if b.kind == crate::Kind::Fs {
            vec![(GetRange, 1)]
        } else {
            vec![]
        };
        let with = |mut base: Vec<(oss_blob::S3Op, u64)>| {
            base.splice(1..1, swap_extra.clone());
            base
        };
        let ((), sent) = counted(b.meter(), || put(&s, "a", "1"));
        assert_eq!(sent, [(Get, 1), (Put, 2)], "first put {:?}", b.kind);
        let (_, sent) = counted(b.meter(), || read(&s, "a"));
        assert_eq!(sent, [(Get, 2)], "get {:?}", b.kind);
        let ((), sent) = counted(b.meter(), || put(&s, "a", "2"));
        assert_eq!(
            sent,
            with(vec![(Get, 1), (Put, 2), (Delete, 1)]),
            "replace {:?}",
            b.kind
        );
        let (_, sent) = counted(b.meter(), || s.names(&mut marker()).unwrap());
        assert_eq!(sent, [(Get, 1)], "names {:?}", b.kind);
        let (_, sent) = counted(b.meter(), || {
            let mut n = 0;
            s.for_each(&mut marker(), &mut body(), &mut |_, _| n += 1)
                .unwrap();
            n
        });
        assert_eq!(sent, [(Get, 2)], "for_each of one {:?}", b.kind);
        let (_, sent) = counted(b.meter(), || s.delete("a", &mut marker()).unwrap());
        assert_eq!(
            sent,
            with(vec![(Get, 1), (Put, 1), (Delete, 1)]),
            "delete {:?}",
            b.kind
        );
        let (_, sent) = counted(b.meter(), || s.delete("a", &mut marker()).unwrap());
        assert_eq!(sent, [(Get, 1)], "delete of none {:?}", b.kind);
    }
}

fn quirky(swaps: u32) -> (Arc<Quirky>, DocSet) {
    let store = Arc::new(Quirky::new(Quirk::ConflictSwaps(swaps)));
    let s = DocSet::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE, KIND).unwrap();
    (store, s)
}

/// A set seeded with `a` through the quirk-free inner store.
fn seeded(swaps: u32) -> (Arc<Quirky>, DocSet) {
    let (store, s) = quirky(swaps);
    let plain = DocSet::new(Arc::clone(&store.inner), BASE, KIND).unwrap();
    put(&plain, "a", "1");
    (store, s)
}

#[test]
fn a_put_retries_a_lost_swap_and_writes_its_bytes_once() {
    let (store, s) = seeded(3);
    // Three lost swaps, refused before the store: four marker reads, the
    // bytes written once, the replaced bytes dropped once.
    let ((), sent) = counted(store.meter(), || put(&s, "a", "2"));
    assert_eq!(sent, [(Get, 4), (Put, 2), (Delete, 1)]);
    assert_eq!(read(&s, "a").as_deref(), Some("2"));
}

#[test]
fn a_put_that_loses_every_attempt_leaves_no_bytes_behind() {
    let (store, s) = seeded(u32::MAX);
    let (got, sent) = counted(store.meter(), || s.put("a", b"2", false, &mut marker()));
    assert!(matches!(got, Err(MetaError::Contended)), "{got:?}");
    assert_eq!(sent, [(Get, 10), (Put, 1), (Delete, 1)]);
    assert_eq!(read(&s, "a").as_deref(), Some("1"));
    let (got, sent) = counted(store.meter(), || s.delete("a", &mut marker()));
    assert!(matches!(got, Err(MetaError::Contended)), "{got:?}");
    assert_eq!(sent, [(Get, 10)]);
}

#[test]
fn a_put_that_finds_the_name_taken_on_its_retry_drops_its_bytes() {
    let store = Arc::new(Quirky::hooked(|inner| {
        // Another writer creates the marker, naming `x`, before ours does.
        let _ = inner.create_register(
            &format!("{BASE}templates/index/_version"),
            br#"{"version":1,"entries":{"x":"theirs"}}"#,
        );
    }));
    let s = DocSet::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE, KIND).unwrap();
    // Requests: the marker read, our bytes, our marker create (hooked).
    store.arm(3);
    let (got, sent) = counted(store.meter(), || s.put("x", b"2", true, &mut marker()));
    assert!(matches!(got, Err(MetaError::AlreadyExists)), "{got:?}");
    // The other writer's create is on the same meter: 3 PUTs, ours twice.
    assert_eq!(sent, [(Get, 2), (Put, 3), (Delete, 1)]);
}

#[test]
fn a_put_whose_marker_write_fails_drops_the_bytes_it_wrote() {
    // Requests: the marker read, our bytes, our marker create (fails).
    let store = Arc::new(Quirky::new(Quirk::FailRequest(3)));
    let s = DocSet::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE, KIND).unwrap();
    let (got, sent) = counted(store.meter(), || s.put("a", b"1", false, &mut marker()));
    assert!(matches!(got, Err(MetaError::Blob(_))), "{got:?}");
    assert_eq!(sent, [(Get, 1), (Put, 1), (Delete, 1)]);
}

/// A set whose marker names a blob that is not there, until a hook, run
/// before the 2nd request, moves the marker to one that is.
pub(crate) fn healing() -> (Arc<Quirky>, DocSet) {
    let store = Arc::new(Quirky::hooked(|inner| {
        let key = format!("{BASE}templates/index/_version");
        let mut buf = [0u8; 256];
        let generation = inner
            .read_register(&key, &mut buf)
            .unwrap()
            .unwrap()
            .generation;
        inner
            .put(
                &format!("{BASE}templates/index/blob-good"),
                &mut SliceSource::new(b"fixed"),
                &mut [0u8; 64],
            )
            .unwrap();
        inner
            .swap_register(&key, generation, br#"{"version":2,"entries":{"a":"good"}}"#)
            .unwrap();
    }));
    store
        .inner
        .create_register(
            &format!("{BASE}templates/index/_version"),
            br#"{"version":1,"entries":{"a":"gone"}}"#,
        )
        .unwrap();
    let s = DocSet::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE, KIND).unwrap();
    store.arm(2);
    (store, s)
}

#[test]
fn a_read_that_meets_a_dropped_document_reads_the_marker_again() {
    let (store, s) = healing();
    let (got, sent) = counted(store.meter(), || read(&s, "a"));
    assert_eq!(got.as_deref(), Some("fixed"));
    // marker, the dropped blob, the marker again, the new blob, and the
    // hook's own marker read, blob and swap on the same meter.
    assert_eq!(sent, [(Get, 5), (Put, 2)]);
}

#[test]
fn a_walk_that_meets_a_dropped_document_starts_again() {
    let (_store, s) = healing();
    let mut seen = vec![];
    s.for_each(&mut marker(), &mut body(), &mut |n, d| {
        seen.push((n.to_owned(), d.to_vec()))
    })
    .unwrap();
    assert_eq!(seen, [("a".to_owned(), b"fixed".to_vec())]);
}

#[test]
fn names_and_documents_at_their_bounds_are_accepted() {
    let b = &all()[0];
    let s = set(b);
    let longest = "n".repeat(255);
    s.put(&longest, &vec![b'x'; MAX_DOC_BYTES], false, &mut marker())
        .unwrap();
    assert!(s.put(&"n".repeat(256), b"x", false, &mut marker()).is_err());
    assert_eq!(s.names(&mut marker()).unwrap(), [longest]);
}
