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
