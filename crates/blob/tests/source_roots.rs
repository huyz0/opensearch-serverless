//! docs/specs/stock-snapshot-restore AC-7: a source outside the allow-lists
//! is refused: a directory outside every root, reached by `..` or a
//! symlink; a bucket and prefix outside every root or overlapping this
//! deployment's own.

use oss_blob::{RootError, SourceRoots};
use oss_testkit::TempDir;

#[test]
fn nothing_is_readable_until_a_root_is_named() {
    let none = SourceRoots::parse("", "");
    assert_eq!(none.fs_location("/tmp"), Err(RootError::Outside));
    assert_eq!(
        none.s3_prefix("b", "p", "own", "x"),
        Err(RootError::Outside)
    );
    // A root that does not exist is skipped.
    let gone = SourceRoots::parse("/no/such/dir", "");
    assert_eq!(gone.fs_location("/no/such/dir"), Err(RootError::Outside));
}

#[test]
fn a_directory_under_a_root_is_allowed_and_the_rest_refused() {
    let dir = TempDir::new("roots");
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("a/b")).unwrap();
    std::fs::create_dir_all(root.join("elsewhere")).unwrap();
    let roots = SourceRoots::parse(&format!("{}/a", root.display()), "");
    let ok = roots.fs_location(&format!("{}/a/b", root.display()));
    assert_eq!(ok.unwrap(), root.join("a/b"));
    assert!(roots.fs_location(&format!("{}/a", root.display())).is_ok());
    assert!(
        roots
            .fs_location(&format!("{}/a/./b//", root.display()))
            .is_ok()
    );
    for outside in [
        format!("{}/elsewhere", root.display()),
        format!("{}/ab", root.display()),
        root.display().to_string(),
    ] {
        assert_eq!(
            roots.fs_location(&outside),
            Err(RootError::Outside),
            "{outside}"
        );
    }
    assert_eq!(
        roots.fs_location(&format!("{}/a/../elsewhere", root.display())),
        Err(RootError::NotAbsolute)
    );
    assert_eq!(roots.fs_location("a/b"), Err(RootError::NotAbsolute));
    assert_eq!(
        roots.fs_location(&format!("{}/a/missing", root.display())),
        Err(RootError::NotDirectory)
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_below_the_root_is_refused() {
    let dir = TempDir::new("roots-link");
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("a/real")).unwrap();
    std::fs::create_dir_all(root.join("secret")).unwrap();
    std::os::unix::fs::symlink(root.join("secret"), root.join("a/link")).unwrap();
    std::os::unix::fs::symlink(root.join("a/real"), root.join("a/alias")).unwrap();
    let roots = SourceRoots::parse(&format!("{}/a", root.display()), "");
    for link in ["link", "alias"] {
        assert_eq!(
            roots.fs_location(&format!("{}/a/{link}", root.display())),
            Err(RootError::Symlink),
            "{link}"
        );
    }
    assert!(
        roots
            .fs_location(&format!("{}/a/real", root.display()))
            .is_ok()
    );
}

#[test]
fn an_s3_source_must_sit_under_a_root() {
    let roots = SourceRoots::parse("", "src-bucket/stock, whole");
    let ok = |bucket: &str, path: &str| roots.s3_prefix(bucket, path, "own", "base");
    assert_eq!(ok("src-bucket", "stock/repo").unwrap(), "stock/repo/");
    assert_eq!(ok("src-bucket", "/stock//repo/./").unwrap(), "stock/repo/");
    assert_eq!(ok("src-bucket", "stock").unwrap(), "stock/");
    assert_eq!(ok("whole", "").unwrap(), "");
    assert_eq!(ok("whole", "any/where").unwrap(), "any/where/");
    for (bucket, path) in [
        ("src-bucket", "other"),
        ("src-bucket", ""),
        ("src-bucket", "stockpile"),
        ("elsewhere", "stock"),
        ("", "stock"),
        ("src-bucket/x", "stock"),
    ] {
        assert_eq!(ok(bucket, path), Err(RootError::Outside), "{bucket}/{path}");
    }
    assert_eq!(
        ok("src-bucket", "stock/../other"),
        Err(RootError::NotAbsolute)
    );
}

#[test]
fn a_source_that_overlaps_the_deployments_own_bucket_is_refused_whatever_the_list_says() {
    let roots = SourceRoots::parse("", "own, own/base/deeper, other");
    let check = |bucket: &str, path: &str| roots.s3_prefix(bucket, path, "own", "base/x");
    // Equal, nested, and a parent, after normalising.
    assert_eq!(check("own", "base/x"), Err(RootError::OwnBucket));
    assert_eq!(check("own", "/base//x/./"), Err(RootError::OwnBucket));
    assert_eq!(check("own", "base/x/deeper"), Err(RootError::OwnBucket));
    assert_eq!(check("own", "base"), Err(RootError::OwnBucket));
    assert_eq!(check("own", ""), Err(RootError::OwnBucket));
    // A sibling prefix of the same bucket is listed and does not overlap.
    assert_eq!(check("own", "base/y").unwrap(), "base/y/");
    assert!(
        check("other", "base/x").is_ok(),
        "another bucket is not own"
    );
    // An own prefix of none overlaps the whole bucket.
    assert_eq!(
        roots.s3_prefix("own", "a", "own", ""),
        Err(RootError::OwnBucket)
    );
}

#[test]
fn every_refusal_reads_as_a_sentence() {
    let all = [
        RootError::Outside,
        RootError::NotAbsolute,
        RootError::Symlink,
        RootError::NotDirectory,
        RootError::OwnBucket,
    ];
    let texts: Vec<String> = all.iter().map(ToString::to_string).collect();
    for (i, t) in texts.iter().enumerate() {
        assert!(t.len() > 20, "{t}");
        assert!(!texts[..i].contains(t), "{t} repeats");
    }
}
