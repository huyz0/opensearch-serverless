//! docs/specs/bucket-manifest-reader/SPEC.md, AC-1 to AC-5 and AC-7.

mod bucket_support;

use bucket_support::{
    DEFLATE, File, LZ4, Len, Loc, Member, NONE, ZSTD, compact, json, sample_commit,
};
use oss_format::{
    BucketEntry, CommitManifest, FileLocation, FormatError, bucket_manifest_key,
    parse_bucket_member,
};

fn member(uuid: &str, index: &str, version: u64, commit: bool) -> Member {
    Member {
        uuid: uuid.to_owned(),
        index: index.to_owned(),
        version,
        commit: commit.then(sample_commit),
    }
}

fn bucket() -> Vec<Member> {
    vec![
        member("aaaa", "alpha", 3, true),
        member("mmmm", "mu-é-日-😀", 1, true),
        member("zzzz", "empty", 0, false),
    ]
}

/// What the sample commit parses to, as (name, location text, length).
fn files_of(commit: &[u8]) -> Vec<(String, String, Option<u64>)> {
    let parsed = CommitManifest::parse(commit).unwrap();
    parsed
        .files()
        .iter()
        .map(|f| (f.name.to_string(), f.location.encode(), f.length))
        .collect()
}

fn expected_files() -> Vec<(String, String, Option<u64>)> {
    let n = "@nodeA/e=";
    let mut files = vec![
        (
            "_0.cfe",
            format!("{n}{:020}/{:020}#0:40", 1_794_710_559_389u64, 4),
            Some(40),
        ),
        (
            "_0.cfs",
            format!("{n}{:020}/{:020}#40:900", 1_794_710_559_389u64, 4),
            Some(900),
        ),
        (
            "_0.si",
            format!("{n}{:020}/{:020}#0:7", 1_794_710_559_390u64, 2),
            Some(7),
        ),
        ("_1.cfe", "t=3/s1#5:11".to_owned(), Some(11)),
        ("segments_4", "t=3".to_owned(), None),
        ("_1.liv", "t=3".to_owned(), Some(19)),
        ("write.lock", "t=3/odd#x:y".to_owned(), None),
    ];
    // Only the first six parse: the raw location is one no reader follows.
    files.truncate(6);
    files.sort_by(|a, b| a.0.cmp(b.0));
    files
        .into_iter()
        .map(|(a, b, c)| (a.to_owned(), b, c))
        .collect()
}

#[test]
fn a_json_manifest_gives_the_named_member_and_its_commit() {
    let commit = r#"{"term":9,"writer":"w","files":{"_0.cfe":"@n/e=00000000000000000001/00000000000000000002#0:40","_0.si":"t=9"},"lengths":{"_0.cfe":40,"_0.si":7},"wal_ordinal":12,"stage_seq":2}"#;
    let bytes = json(
        "b-1",
        5,
        &[
            (
                "aaaa",
                "other",
                Some(2),
                Some(r#"{"term":1,"files":{"x":{"not":"read"}}}"#),
            ),
            ("mmmm", "mine", Some(4), Some(commit)),
            ("zzzz", "none", None, None),
        ],
    );
    let entry = parse_bucket_member(&bytes, "mmmm").unwrap();
    assert_eq!((entry.bucket.as_str(), entry.term), ("b-1", 5));
    let member = entry.member.unwrap();
    assert_eq!((member.index_name.as_str(), member.version), ("mine", 4));
    let commit_bytes = member.commit.unwrap();
    assert_eq!(commit_bytes, commit.as_bytes());
    let parsed = CommitManifest::parse(&commit_bytes).unwrap();
    assert_eq!(
        (parsed.term, parsed.wal_ordinal, parsed.stage_sequence),
        (9, 12, 2)
    );
    assert_eq!(parsed.writer.as_deref(), Some("w"));
    assert_eq!(parsed.files().len(), 2);
    // Another member's malformed commit is never read.
    assert!(parse_bucket_member(&bytes, "aaaa").is_err());
}

#[test]
fn a_json_member_with_no_commit_has_version_zero_and_a_given_version_must_agree() {
    let bytes = json("b", 1, &[("u", "idx", None, None)]);
    let member = parse_bucket_member(&bytes, "u").unwrap().member.unwrap();
    assert_eq!((member.version, member.commit), (0, None));
    let disagree = json("b", 1, &[("u", "idx", Some(2), None)]);
    assert!(matches!(
        parse_bucket_member(&disagree, "u"),
        Err(FormatError::Corrupt { .. })
    ));
    let lone = json(
        "b",
        1,
        &[("u", "idx", Some(0), Some(r#"{"term":1,"files":{}}"#))],
    );
    assert!(matches!(
        parse_bucket_member(&lone, "u"),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_compact_manifest_gives_the_named_member_with_every_field_the_java_wrote() {
    let bytes = compact(NONE, "b-1", 5, &bucket());
    let entry = parse_bucket_member(&bytes, "mmmm").unwrap();
    assert_eq!((entry.bucket.as_str(), entry.term), ("b-1", 5));
    let member = entry.member.unwrap();
    assert_eq!(
        (member.index_name.as_str(), member.version),
        ("mu-é-日-😀", 1)
    );
    let commit = member.commit.unwrap();
    let parsed = CommitManifest::parse(&commit);
    // The sample holds a raw location that is no reader's to follow, so the commit
    // parse refuses it: the reader gave the bytes through and the parse judged them.
    assert!(matches!(parsed, Err(FormatError::Corrupt { .. })));
    let text = std::str::from_utf8(&commit).unwrap();
    assert!(text.contains("\"write.lock\":\"t=3/odd#x:y\""));
    assert!(text.contains("\"writer\":\"tZ9iGQAAQACwLK7Z_____w\""));
    assert!(text.contains("\"wal_ordinal\":41,\"stage_seq\":3"));
    assert!(text.contains("\"term\":7"));
}

fn clean_bucket() -> Vec<Member> {
    let mut commit = sample_commit();
    commit.files.retain(|f| f.name != "write.lock");
    vec![
        Member {
            uuid: "aaaa".into(),
            index: "alpha".into(),
            version: 3,
            commit: Some(commit.clone()),
        },
        Member {
            uuid: "mmmm".into(),
            index: "mu-é-日-😀".into(),
            version: 1,
            commit: Some(commit),
        },
        member("zzzz", "empty", 0, false),
    ]
}

#[test]
fn a_compact_commit_parses_to_the_files_ranges_and_lengths_that_were_written() {
    let bytes = compact(NONE, "b-1", 5, &clean_bucket());
    let commit = parse_bucket_member(&bytes, "mmmm")
        .unwrap()
        .member
        .unwrap()
        .commit
        .unwrap();
    let parsed = CommitManifest::parse(&commit).unwrap();
    assert_eq!(parsed.term, 7);
    assert_eq!(parsed.writer.as_deref(), Some("tZ9iGQAAQACwLK7Z_____w"));
    assert_eq!((parsed.wal_ordinal, parsed.stage_sequence), (41, 3));
    assert_eq!(files_of(&commit), expected_files());
    let cfs = parsed.file("_0.cfs").unwrap();
    assert!(matches!(
        cfs.location,
        FileLocation::NodePacked {
            node: "nodeA",
            epoch: 1_794_710_559_389,
            ordinal: 4,
            offset: 40,
            length: 900
        }
    ));
    assert_eq!(parsed.file("segments_4").unwrap().length, None);
}

#[test]
fn every_codec_gives_the_member_the_uncompressed_form_does() {
    let members = clean_bucket();
    let plain = parse_bucket_member(&compact(NONE, "b-1", 5, &members), "aaaa").unwrap();
    for codec in [DEFLATE, ZSTD, LZ4] {
        let entry = parse_bucket_member(&compact(codec, "b-1", 5, &members), "aaaa").unwrap();
        assert_eq!(entry, plain, "codec {codec}");
    }
}

#[test]
fn a_large_bucket_reads_through_every_codec_and_lz4_uses_its_matches() {
    let members: Vec<Member> = (0..1000)
        .map(|i| member(&format!("uuid-{i:05}"), &format!("index-{i}"), 1, false).with_commit(i))
        .collect();
    let plain = compact(NONE, "big", 9, &members);
    let packed = compact(LZ4, "big", 9, &members);
    assert!(
        packed.len() < plain.len() / 2,
        "the repeats compress: {} of {}",
        packed.len(),
        plain.len()
    );
    for codec in [NONE, DEFLATE, ZSTD, LZ4] {
        let bytes = compact(codec, "big", 9, &members);
        let entry = parse_bucket_member(&bytes, "uuid-00707").unwrap();
        let found = entry.member.unwrap();
        assert_eq!((found.index_name.as_str(), found.version), ("index-707", 1));
        let commit_bytes = found.commit.unwrap();
        let commit = CommitManifest::parse(&commit_bytes).unwrap();
        assert_eq!(commit.term, 707, "codec {codec}");
    }
}

trait WithCommit {
    fn with_commit(self, term: u64) -> Self;
}

impl WithCommit for Member {
    fn with_commit(mut self, term: u64) -> Self {
        let mut commit = sample_commit();
        commit.files.retain(|f| f.name != "write.lock");
        commit.term = term;
        commit.files.push(File {
            name: "_9.dvd",
            loc: Loc::Node {
                node: "nodeB",
                epoch: 3,
                ordinal: 8,
                offset: 0,
                length: 12,
            },
            len: Len::OfRange,
        });
        self.commit = Some(commit);
        self.version = 1;
        self
    }
}

#[test]
fn an_unknown_uuid_gives_the_bucket_and_term_and_no_member() {
    for bytes in [
        compact(ZSTD, "b-1", 5, &clean_bucket()),
        json("b-1", 5, &[("u", "i", None, None)]),
    ] {
        let entry = parse_bucket_member(&bytes, "nobody").unwrap();
        assert_eq!(
            entry,
            BucketEntry {
                bucket: "b-1".to_owned(),
                term: 5,
                member: None
            }
        );
    }
}

#[test]
fn a_compact_member_with_no_commit_has_version_zero() {
    let bytes = compact(NONE, "b-1", 5, &clean_bucket());
    let member = parse_bucket_member(&bytes, "zzzz").unwrap().member.unwrap();
    assert_eq!(
        (member.index_name.as_str(), member.version, member.commit),
        ("empty", 0, None)
    );
}

#[test]
fn the_register_key_is_buckets_id_manifest_under_the_base() {
    assert_eq!(
        bucket_manifest_key("", "b-1_x").unwrap(),
        "buckets/b-1_x/manifest"
    );
    assert_eq!(
        bucket_manifest_key("dep/", "b").unwrap(),
        "dep/buckets/b/manifest"
    );
    let long = "a".repeat(65);
    for bad in ["", "A", "a/b", "a b", "é", long.as_str()] {
        assert!(bucket_manifest_key("", bad).is_err(), "{bad:?}");
    }
    assert!(bucket_manifest_key("", &"a".repeat(64)).is_ok());
    assert!(bucket_manifest_key("dep", "b").is_err());
}
