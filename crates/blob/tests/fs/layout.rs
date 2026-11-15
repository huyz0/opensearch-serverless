//! Reads, HEAD, DELETE, key layout and argument checks.

use std::path::Path;

use oss_blob::{BlobError, BlobStore, ByteRange, FsStore, S3Op, SliceSource};

use super::{assert_counts, get_all, list_all, pattern, put, store};

#[test]
fn each_operation_records_exactly_one_attempt_of_its_kind() {
    let (s, _dir) = store("fs-counts");
    put(&s, "a/k", b"hello");
    assert_counts(s.meter(), &[(S3Op::Put, 1)]);
    get_all(&s, "a/k", ByteRange::Full).unwrap();
    get_all(&s, "a/k", ByteRange::From { start: 1, len: 2 }).unwrap();
    assert!(s.head("a/k").unwrap().is_some());
    s.create_if_absent("a/n", &mut SliceSource::new(b"x"), &mut [0; 8])
        .unwrap();
    // LIST: the operation under test.
    s.list_prefix("a/", None, 10, &mut |_, _| {}).unwrap();
    s.delete("a/k").unwrap();
    assert_counts(
        s.meter(),
        &[
            (S3Op::Put, 2),
            (S3Op::Get, 1),
            (S3Op::GetRange, 1),
            (S3Op::Head, 1),
            (S3Op::List, 1),
            (S3Op::Delete, 1),
        ],
    );
    assert_eq!(s.meter().errors(), 0);
    assert_eq!(s.meter().bytes_written(), 6);
    assert_eq!(s.meter().bytes_read(), 7);
}

#[test]
fn a_failed_call_is_one_attempt_and_one_error() {
    let (s, _dir) = store("fs-fail");
    assert_eq!(
        get_all(&s, "missing", ByteRange::Full),
        Err(BlobError::NotFound)
    );
    assert_counts(s.meter(), &[(S3Op::Get, 1)]);
    assert_eq!(s.meter().errors(), 1);
}

#[test]
#[allow(clippy::disallowed_methods)] // one small file, read whole to compare
fn a_key_is_a_file_under_the_root_holding_exactly_the_body() {
    let (s, _dir) = store("fs-layout");
    put(&s, "segments/idx#uuid#0/t=2/_1.seg", b"segment");
    let file = s
        .root()
        .join("segments")
        .join("idx#uuid#0")
        .join("t=2")
        .join("_1.seg");
    assert_eq!(std::fs::read(file).unwrap(), b"segment");
}

/// The blobs the Java shell's `FsBlobStore` wrote, read byte for byte and
/// listed whole.
#[test]
#[allow(clippy::disallowed_methods)] // small fixture files, read whole to compare
fn a_directory_the_java_shell_wrote_reads_and_lists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/shell-blobs");
    let s = FsStore::new(&root).unwrap();
    let mut want = Vec::new();
    for top in ["indices", "packs", "segments"] {
        collect(&root, &root.join(top), &mut want);
    }
    want.sort();
    assert!(want.len() >= 4, "{want:?}");
    let listed: Vec<String> = list_all(&s, "", 2).into_iter().map(|(k, _)| k).collect();
    let listed: Vec<String> = listed.into_iter().filter(|k| !k.ends_with(".md")).collect();
    assert_eq!(listed, want);
    for key in &want {
        let on_disk = std::fs::read(root.join(key)).unwrap();
        assert_eq!(get_all(&s, key, ByteRange::Full).unwrap(), on_disk, "{key}");
        assert_eq!(s.head(key).unwrap().unwrap().len, on_disk.len() as u64);
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap();
            let parts: Vec<_> = rel.iter().map(|p| p.to_str().unwrap()).collect();
            out.push(parts.join("/"));
        }
    }
}

#[test]
fn a_ranged_get_returns_exactly_the_bytes_asked_for() {
    let (s, _dir) = store("fs-range");
    let body = pattern(2500);
    put(&s, "k", &body);
    assert_eq!(
        get_all(
            &s,
            "k",
            ByteRange::From {
                start: 10,
                len: 1500
            }
        )
        .unwrap(),
        body[10..1510]
    );
    // Truncated at the end, as S3 does.
    assert_eq!(
        get_all(
            &s,
            "k",
            ByteRange::From {
                start: 2400,
                len: 500
            }
        )
        .unwrap(),
        body[2400..]
    );
    assert_eq!(
        get_all(
            &s,
            "k",
            ByteRange::From {
                start: 2499,
                len: 1
            }
        )
        .unwrap(),
        body[2499..]
    );
    assert_eq!(
        get_all(
            &s,
            "k",
            ByteRange::From {
                start: 2500,
                len: 1
            }
        ),
        Err(BlobError::InvalidRange)
    );
    put(&s, "empty", b"");
    assert_eq!(get_all(&s, "empty", ByteRange::Full).unwrap(), b"");
    assert_eq!(
        get_all(&s, "empty", ByteRange::From { start: 0, len: 1 }),
        Err(BlobError::InvalidRange)
    );
}

#[test]
fn a_get_streams_through_the_caller_buffer() {
    let (s, _dir) = store("fs-chunks");
    let body = pattern(10_000);
    put(&s, "k", &body);
    let mut buf = [0u8; 333];
    let mut chunks = 0usize;
    let mut out = Vec::new();
    let got = s
        .get("k", ByteRange::Full, &mut buf, &mut |c: &[u8]| {
            assert!(!c.is_empty() && c.len() <= 333);
            chunks += 1;
            out.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
    assert_eq!(out, body);
    assert_eq!((got.object_len, got.bytes), (10_000, 10_000));
    assert!(chunks >= 10_000 / 333);
}

#[test]
fn a_sink_failure_ends_the_get_and_counts_as_an_error() {
    let (s, _dir) = store("fs-sink");
    put(&s, "k", &pattern(5000));
    let mut calls = 0;
    let r = s.get("k", ByteRange::Full, &mut [0; 100], &mut |_: &[u8]| {
        calls += 1;
        Err(BlobError::Stream("closed"))
    });
    assert_eq!(r, Err(BlobError::Stream("closed")));
    assert_eq!(calls, 1);
    assert_eq!(s.meter().errors(), 1);
}

#[test]
fn a_directory_is_no_object() {
    let (s, _dir) = store("fs-dir");
    put(&s, "a/b/c", b"x");
    assert_eq!(
        get_all(&s, "a/b", ByteRange::Full),
        Err(BlobError::NotFound)
    );
    assert_eq!(s.head("a/b").unwrap(), None);
    assert_eq!(s.delete("a/b"), Ok(()));
    assert!(s.head("a/b/c").unwrap().is_some());
    // A file on the way to a key is no directory either.
    assert_eq!(
        get_all(&s, "a/b/c/d", ByteRange::Full),
        Err(BlobError::NotFound)
    );
    assert_eq!(s.head("a/b/c/d").unwrap(), None);
    assert_eq!(s.delete("a/b/c/d"), Ok(()));
}

#[test]
fn head_and_delete_follow_s3() {
    let (s, _dir) = store("fs-head");
    assert_eq!(s.head("k").unwrap(), None);
    assert_eq!(s.delete("k"), Ok(()));
    put(&s, "k", b"abc");
    assert_eq!(s.head("k").unwrap().unwrap().len, 3);
    s.delete("k").unwrap();
    assert_eq!(s.head("k").unwrap(), None);
    assert_eq!(get_all(&s, "k", ByteRange::Full), Err(BlobError::NotFound));
    assert_eq!(s.meter().errors(), 1);
}

#[test]
fn invalid_arguments_fail_before_any_attempt() {
    let (s, _dir) = store("fs-args");
    let long = "k".repeat(1025);
    let long_part = format!("a/{}", "c".repeat(256));
    let bad_keys = [
        "",
        long.as_str(),
        long_part.as_str(),
        "a//b",
        "a/",
        "./a",
        "a/..",
        "a\\b",
        "a\0b",
        "x/.opensearch-register-locks#/y",
    ];
    for key in bad_keys {
        let invalid = |r: Result<(), BlobError>| matches!(r, Err(BlobError::InvalidArgument(_)));
        assert!(
            invalid(get_all(&s, key, ByteRange::Full).map(|_| ())),
            "get {key:?}"
        );
        assert!(
            invalid(s.put(key, &mut SliceSource::new(b"x"), &mut [0; 4])),
            "put {key:?}"
        );
        assert!(
            invalid(
                s.create_if_absent(key, &mut SliceSource::new(b"x"), &mut [0; 4])
                    .map(|_| ())
            ),
            "create {key:?}"
        );
        assert!(invalid(s.head(key).map(|_| ())), "head {key:?}");
        assert!(invalid(s.delete(key)), "delete {key:?}");
    }
    let invalid = |r: Result<(), BlobError>| matches!(r, Err(BlobError::InvalidArgument(_)));
    assert!(invalid(s.put("k", &mut SliceSource::new(b"x"), &mut [])));
    assert!(invalid(
        s.get("k", ByteRange::Full, &mut [], &mut |_: &[u8]| Ok(()))
            .map(|_| ())
    ));
    assert!(invalid(
        s.get(
            "k",
            ByteRange::From { start: 0, len: 0 },
            &mut [0; 4],
            &mut |_: &[u8]| Ok(())
        )
        .map(|_| ())
    ));
    assert!(invalid(
        // LIST: the operation under test.
        s.list_prefix("", None, 0, &mut |_, _| {}).map(|_| ())
    ));
    assert_eq!(s.meter().total(), 0);
    assert_eq!(s.meter().errors(), 0);
}

/// A filesystem failure that is no missing key is a store failure, one
/// attempt and one error each: a symbolic link to itself cannot be opened,
/// read through, listed or removed through.
#[cfg(unix)]
#[test]
fn a_filesystem_failure_is_a_failed_attempt() {
    let (s, _dir) = store("fs-loop");
    std::os::unix::fs::symlink("loop", s.root().join("loop")).unwrap();
    assert_eq!(
        get_all(&s, "loop", ByteRange::Full),
        Err(BlobError::Unavailable)
    );
    assert_eq!(s.head("loop"), Err(BlobError::Unavailable));
    assert_eq!(s.delete("loop/k"), Err(BlobError::Unavailable));
    // LIST: the operation under test.
    let listed = s.list_prefix("loop/", None, 10, &mut |_, _| {});
    assert_eq!(listed, Err(BlobError::Unavailable));
    // A loop inside a listed directory fails the page too.
    std::fs::create_dir(s.root().join("x")).unwrap();
    std::os::unix::fs::symlink("loop", s.root().join("x").join("loop")).unwrap();
    // LIST: the operation under test.
    let listed = s.list_prefix("x/", None, 10, &mut |_, _| {});
    assert_eq!(listed, Err(BlobError::Unavailable));
    assert_counts(
        s.meter(),
        &[
            (S3Op::Get, 1),
            (S3Op::Head, 1),
            (S3Op::Delete, 1),
            (S3Op::List, 2),
        ],
    );
    assert_eq!(s.meter().errors(), 5);
}
