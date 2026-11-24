//! docs/specs/bucket-java-fixtures/SPEC.md: bucket manifests the Java shell
//! wrote are read, and the JSON form is written back byte for byte.

// ARITH: test sizes are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{BucketManifest, CommitManifest, parse_bucket_member};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bucket-manifests")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn check(manifest: &BucketManifest) {
    assert_eq!((manifest.bucket(), manifest.term()), ("s01-2", 9));
    let uuids: Vec<&str> = manifest.members().map(|(uuid, _)| uuid).collect();
    assert_eq!(uuids, ["uuid-a", "uuid-b", "uuid-c"]);
    let a = manifest.member("uuid-a").expect("a");
    assert_eq!((a.index_name.as_str(), a.version), ("logs-a", 5));
    let commit = CommitManifest::parse(a.commit.as_ref().expect("commit")).expect("parses");
    assert_eq!(commit.term, 4);
    assert_eq!(commit.writer.as_deref(), Some("AlpgfwAAQACoTLx8AAAAAA"));
    assert_eq!(commit.wal_ordinal, 77);
    assert_eq!(commit.stage_sequence, 12);
    let names: Vec<&str> = commit.files().iter().map(|f| &*f.name).collect();
    assert_eq!(
        names,
        [
            "_0.cfe",
            "_0.cfs",
            "_1.si",
            "_2_Lucene90_0.dvd",
            "_3.cfs",
            "segments_a"
        ]
    );
    let b = manifest.member("uuid-b").expect("b");
    assert_eq!((b.version, b.commit.is_none()), (0, true));
    let c = manifest.member("uuid-c").expect("c");
    assert_eq!(c.version, 1);
    let c = CommitManifest::parse(c.commit.as_ref().expect("commit")).expect("parses");
    assert_eq!((c.term, c.writer), (1, None));
}

#[test]
fn the_java_json_manifest_is_read_and_written_back_byte_for_byte() {
    let bytes = fixture("bucket.json");
    let manifest = BucketManifest::parse(&bytes).expect("parses");
    check(&manifest);
    assert_eq!(manifest.encode().expect("encodes"), bytes);
}

#[test]
fn every_compact_codec_the_java_wrote_is_read_to_the_same_manifest() {
    for name in [
        "bucket-none.bin",
        "bucket-deflate.bin",
        "bucket-zstd.bin",
        "bucket-lz4_fast.bin",
    ] {
        let bytes = fixture(name);
        check(&BucketManifest::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}")));
        let one = parse_bucket_member(&bytes, "uuid-a").unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((one.bucket.as_str(), one.term), ("s01-2", 9));
        assert_eq!(one.member.expect("member").version, 5);
        assert!(
            parse_bucket_member(&bytes, "uuid-zzz")
                .expect("parses")
                .member
                .is_none()
        );
    }
}

#[test]
fn the_json_and_compact_forms_read_to_one_manifest() {
    let json = BucketManifest::parse(&fixture("bucket.json")).expect("json");
    let compact = BucketManifest::parse(&fixture("bucket-none.bin")).expect("compact");
    let members = |m: &BucketManifest| -> Vec<(String, String, u64, Option<String>)> {
        m.members()
            .map(|(uuid, member)| {
                let commit = member
                    .commit
                    .as_ref()
                    .map(|c| format!("{:?}", CommitManifest::parse(c).expect("parses").files()));
                (
                    uuid.to_owned(),
                    member.index_name.clone(),
                    member.version,
                    commit,
                )
            })
            .collect()
    };
    assert_eq!(members(&json), members(&compact));
}

#[test]
fn a_recorded_length_that_disagrees_with_its_range_is_refused_where_the_java_reads_it() {
    let err = BucketManifest::parse(&fixture("length-disagrees.json")).expect_err("refused");
    assert!(
        err.to_string().contains("disagrees with the file's range"),
        "{err}"
    );
}

#[test]
fn the_register_keys_are_the_javas() {
    // `BucketStore.ROOT`/`OPEN_ROOT` and `BucketIds.validate` in the shell.
    assert_eq!(
        oss_format::bucket_manifest_key("", "s01-2").unwrap(),
        "buckets/s01-2/manifest"
    );
    assert_eq!(
        oss_format::bucket_manifest_key("base/", "s01-2").unwrap(),
        "base/buckets/s01-2/manifest"
    );
    assert_eq!(oss_format::bucket_open_key("", 7).unwrap(), "bucket-open/7");
    assert!(oss_format::bucket_manifest_key("", "").is_err());
    assert!(oss_format::bucket_manifest_key("", "Bad").is_err());
    assert!(oss_format::bucket_manifest_key("", &"a".repeat(65)).is_err());
    assert!(oss_format::bucket_manifest_key("", &"a".repeat(64)).is_ok());
}
