//! LIST pages: S3's flat byte order over nested directories.

use oss_blob::{BlobStore, S3Op};

use super::{assert_counts, list_all, put, store};

#[test]
fn a_page_is_the_next_keys_in_whole_key_byte_order() {
    let (s, _dir) = store("fs-order");
    // '-' (0x2d) sorts before '/' (0x2f), and '0' (0x30) after it, so
    // directory order by name alone would put a/b/c in the wrong place.
    let keys = ["a/b-c", "a/b/c", "a/b/d/e", "a/b0", "a/ba", "b", "a/b"];
    // "a/b" cannot be both a file and a directory: leave it out.
    for k in keys.iter().filter(|k| **k != "a/b") {
        put(&s, k, k.as_bytes());
    }
    let want = ["a/b-c", "a/b/c", "a/b/d/e", "a/b0", "a/ba", "b"];
    let all = list_all(&s, "", 1000);
    let names: Vec<&str> = all.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, want);
    for (k, len) in &all {
        assert_eq!(*len, k.len() as u64);
    }
    // One key a page visits the same keys in the same order.
    let one: Vec<String> = list_all(&s, "", 1).into_iter().map(|(k, _)| k).collect();
    assert_eq!(one, want);
    // A prefix that ends inside a name, and one that ends at a directory.
    let b: Vec<String> = list_all(&s, "a/b", 1000)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(b, ["a/b-c", "a/b/c", "a/b/d/e", "a/b0", "a/ba"]);
    let dir: Vec<String> = list_all(&s, "a/b/", 1000)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(dir, ["a/b/c", "a/b/d/e"]);
    let none = list_all(&s, "a/b/d/e/", 1000);
    assert!(none.is_empty());
    assert!(list_all(&s, "zzz", 10).is_empty());
    assert!(list_all(&s, "a//", 10).is_empty());
}

#[test]
fn a_page_starts_after_the_cursor() {
    let (s, _dir) = store("fs-cursor");
    for k in ["x/1", "x/2/a", "x/2/b", "x/3"] {
        put(&s, k, b"v");
    }
    let mut seen = Vec::new();
    let page = s
        // LIST: the operation under test.
        .list_prefix("x/", Some("x/2/a"), 10, &mut |k, _| seen.push(k.to_owned()))
        .unwrap();
    assert_eq!(seen, ["x/2/b", "x/3"]);
    assert!(!page.truncated);
    seen.clear();
    // A cursor that is no key, between two directories' keys.
    // LIST: the operation under test.
    s.list_prefix("x/", Some("x/2/zz"), 10, &mut |k, _| {
        seen.push(k.to_owned())
    })
    .unwrap();
    assert_eq!(seen, ["x/3"]);
    seen.clear();
    // A cursor before the prefix lists from its start.
    // LIST: the operation under test.
    s.list_prefix("x/", Some("a"), 2, &mut |k, _| seen.push(k.to_owned()))
        .unwrap();
    assert_eq!(seen, ["x/1", "x/2/a"]);
}

#[test]
fn listing_pages_through_2500_keys_in_3_lists() {
    let (s, _dir) = store("fs-pages");
    for i in 0..2500 {
        put(&s, &format!("k/{:02}/{i:04}", i % 7), b"");
    }
    let before = s.meter().count(S3Op::List);
    let all = list_all(&s, "k/", 5000);
    assert_eq!(s.meter().count(S3Op::List) - before, 3);
    assert_eq!(all.len(), 2500);
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted);
    sorted.dedup();
    assert_eq!(sorted.len(), 2500);
}

#[test]
fn the_reserved_directory_is_never_listed() {
    let (s, _dir) = store("fs-reserved");
    put(&s, "r/k", b"v");
    let reserved = s.root().join("r").join(".opensearch-register-locks#");
    std::fs::create_dir_all(&reserved).unwrap();
    std::fs::write(reserved.join("0"), b"lock").unwrap();
    std::fs::write(reserved.join("pending-left-behind"), b"part").unwrap();
    let keys: Vec<String> = list_all(&s, "", 1000).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["r/k"]);
    assert_counts(s.meter(), &[(S3Op::Put, 1), (S3Op::List, 1)]);
}

/// The walk stops where no key can live: a directory 64 components down
/// holds only 65-component names. A link to itself there would fail the
/// LIST if the walk entered it.
#[cfg(unix)]
#[test]
fn the_walk_stops_at_the_deepest_key() {
    let (s, _dir) = store("fs-deep");
    let parts: Vec<String> = (0..64).map(|i| format!("d{i}")).collect();
    let deepest_key = parts[..63].join("/") + "/k";
    put(&s, &deepest_key, b"v");
    let mut too_deep = s.root().to_path_buf();
    too_deep.extend(&parts);
    std::fs::create_dir_all(&too_deep).unwrap();
    std::os::unix::fs::symlink("loop", too_deep.join("loop")).unwrap();
    let keys: Vec<String> = list_all(&s, "", 1000).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, [deepest_key]);
}

/// Entries that cannot be keys: a name that is not UTF-8, a link to
/// nothing, and a socket, which is neither file nor directory.
#[cfg(unix)]
#[test]
fn entries_that_are_not_objects_are_skipped() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let (s, _dir) = store("fs-odd");
    put(&s, "o/k", b"v");
    let dir = s.root().join("o");
    std::fs::write(dir.join(OsStr::from_bytes(b"bad\xff")), b"v").unwrap();
    std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("dangling")).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(dir.join("sock")).unwrap();
    // A link to an object lists as the object, as the Java's
    // `Files.readAttributes` follows it.
    std::os::unix::fs::symlink(dir.join("k"), dir.join("link")).unwrap();
    let keys: Vec<String> = list_all(&s, "o/", 1000)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, ["o/k", "o/link"]);
}

/// A link to itself in `dir`, which fails a LIST that walks `dir`.
#[cfg(unix)]
fn trap(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::os::unix::fs::symlink("loop", dir.join("loop")).unwrap();
}

/// Lists one page and returns its keys and whether it was truncated.
fn one_page(
    s: &dyn BlobStore,
    prefix: &str,
    after: Option<&str>,
    max: usize,
) -> (Vec<String>, bool) {
    let mut seen = Vec::new();
    // LIST: the operation under test.
    let page = s
        .list_prefix(prefix, after, max, &mut |k, _| seen.push(k.to_owned()))
        .unwrap();
    (seen, page.truncated)
}

/// A page never walks a directory none of whose keys could enter it: one
/// before the cursor, or one too long to hold a key. Skipping a directory
/// past a full page depends on the order the directory is read in, so no
/// test can pin it; it saves work, never changes a page.
#[cfg(unix)]
#[test]
fn a_page_never_walks_a_directory_that_cannot_enter_it() {
    let (s, _dir) = store("fs-prune");
    for k in ["m/1", "m/2", "m/3"] {
        put(&s, k, b"v");
    }
    trap(&s.root().join("a"));
    let mut long = s.root().join("l");
    for _ in 0..5 {
        long.push("d".repeat(255));
    }
    trap(&long);
    let two = vec!["m/1".to_owned(), "m/2".to_owned()];
    assert_eq!(one_page(&s, "m", None, 2), (two.clone(), true));
    assert_eq!(one_page(&s, "", Some("b"), 2), (two, true));
    assert_eq!(one_page(&s, "l", None, 1000), (Vec::new(), false));
}

#[test]
fn a_page_of_exactly_the_remaining_keys_is_not_truncated() {
    let (s, _dir) = store("fs-exact");
    for k in ["e/1", "e/2"] {
        put(&s, k, b"v");
    }
    // LIST: the operation under test.
    let page = s.list_prefix("e/", None, 2, &mut |_, _| {}).unwrap();
    assert_eq!((page.keys, page.truncated), (2, false));
}
