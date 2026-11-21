//! docs/specs/bucket-manifest-writer/SPEC.md, AC-1 to AC-7.

use oss_format::{
    BucketManifest, BucketMember, FormatError, MAX_BUCKET_MEMBERS, ManifestEntry, encode_manifest,
    parse_bucket_member,
};

fn commit(term: u64, lengths: &[(&str, u64)]) -> Vec<u8> {
    let files: Vec<ManifestEntry<'_>> = lengths
        .iter()
        .map(|(name, length)| ManifestEntry {
            name,
            location: format!("t={term}"),
            length: *length,
        })
        .collect();
    encode_manifest(term, Some("nodeA"), &files, 5, -1).unwrap()
}

fn sample() -> BucketManifest {
    BucketManifest::empty("b-1")
        .unwrap()
        .at_term(9)
        .unwrap()
        .with_member(
            "uuid-b",
            BucketMember::first("beta", Some(commit(2, &[("_0.cfs", 100), ("_0.si", 7)]))).unwrap(),
        )
        .unwrap()
        .with_member("uuid-a", BucketMember::first("al\"pha", None).unwrap())
        .unwrap()
}

#[test]
fn a_manifest_round_trips_in_uuid_order() {
    let manifest = sample();
    let bytes = manifest.encode().unwrap();
    assert_eq!(BucketManifest::parse(&bytes).unwrap(), manifest);
    let uuids: Vec<&str> = manifest.members().map(|(uuid, _)| uuid).collect();
    assert_eq!(uuids, ["uuid-a", "uuid-b"]);
}

#[test]
fn the_bytes_are_the_shells_json_shape_and_each_member_reads_back() {
    let manifest = sample();
    let bytes = manifest.encode().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let c = String::from_utf8(commit(2, &[("_0.cfs", 100), ("_0.si", 7)])).unwrap();
    let expected = format!(
        "{{\"bucket\":\"b-1\",\"term\":9,\"members\":{{\"uuid-a\":{{\"index\":\"al\\\"pha\"}},\"uuid-b\":{{\"index\":\"beta\",\"version\":1,\"commit\":{c}}}}}}}"
    );
    assert_eq!(text, expected);
    let a = parse_bucket_member(&bytes, "uuid-a").unwrap();
    assert_eq!((a.bucket.as_str(), a.term), ("b-1", 9));
    let a = a.member.unwrap();
    assert_eq!(
        (a.index_name.as_str(), a.version, a.commit),
        ("al\"pha", 0, None)
    );
    let b = parse_bucket_member(&bytes, "uuid-b")
        .unwrap()
        .member
        .unwrap();
    assert_eq!(b.version, 1);
    assert_eq!(
        b.commit.unwrap(),
        commit(2, &[("_0.cfs", 100), ("_0.si", 7)])
    );
}

#[test]
fn a_manifest_the_java_wrote_parses_whole_in_any_field_order() {
    let c = String::from_utf8(commit(3, &[("a", 1)])).unwrap();
    let text = format!(
        "{{\"extra\":{{\"x\":[1,2]}},\"members\":{{\"u2\":{{\"commit\":{c},\"version\":4,\"index\":\"two\",\"more\":[1]}},\"u1\":{{\"index\":\"one\"}}}},\"term\":12,\"bucket\":\"bk\"}}"
    );
    let manifest = BucketManifest::parse(text.as_bytes()).unwrap();
    assert_eq!((manifest.bucket(), manifest.term()), ("bk", 12));
    for uuid in ["u1", "u2"] {
        let single = parse_bucket_member(text.as_bytes(), uuid).unwrap().member;
        assert_eq!(manifest.member(uuid), single.as_ref());
    }
    assert_eq!(manifest.member("u2").unwrap().version, 4);
    assert!(manifest.member("u3").is_none());
}

#[test]
fn edits_change_one_member_and_leave_the_rest() {
    let base = sample();
    let replaced = base
        .clone()
        .with_member("uuid-a", BucketMember::first("renamed", None).unwrap())
        .unwrap();
    assert_eq!(replaced.member("uuid-a").unwrap().index_name, "renamed");
    assert_eq!(replaced.member("uuid-b"), base.member("uuid-b"));
    let removed = base.clone().without_member("uuid-b");
    assert!(removed.member("uuid-b").is_none());
    assert_eq!(removed.member("uuid-a"), base.member("uuid-a"));
    assert_eq!(base.clone().without_member("nope"), base);
    let moved = base.clone().at_term(10).unwrap();
    assert_eq!(moved.term(), 10);
    assert_eq!(moved.member("uuid-b"), base.member("uuid-b"));
}

#[test]
fn versions_move_only_with_a_commit() {
    assert_eq!(BucketMember::first("i", None).unwrap().version, 0);
    let first = BucketMember::first("i", Some(commit(1, &[("a", 1)]))).unwrap();
    assert_eq!(first.version, 1);
    let next = first.advance(commit(2, &[("a", 2)])).unwrap();
    assert_eq!(next.version, 2);
    assert_eq!(
        next.commit.as_deref(),
        Some(commit(2, &[("a", 2)]).as_slice())
    );
    let no_commit_with_version =
        br#"{"bucket":"b","term":1,"members":{"u":{"index":"i","version":2}}}"#;
    assert!(BucketManifest::parse(no_commit_with_version).is_err());
}

#[test]
fn data_bytes_sums_recorded_lengths() {
    assert_eq!(sample().data_bytes().unwrap(), 107);
    assert_eq!(BucketManifest::empty("b").unwrap().data_bytes().unwrap(), 0);
}

#[test]
fn refusals_are_errors() {
    // A compact header with no body.
    assert!(BucketManifest::parse(b"OSBM\x02\x00\x00\x00\x00\x00").is_err());
    // Too many members.
    let mut manifest = BucketManifest::empty("b").unwrap();
    for i in 0..MAX_BUCKET_MEMBERS {
        manifest = manifest
            .with_member(&format!("u{i:05}"), BucketMember::first("i", None).unwrap())
            .unwrap();
    }
    assert!(matches!(
        manifest
            .clone()
            .with_member("extra", BucketMember::first("i", None).unwrap()),
        Err(FormatError::TooLarge { .. })
    ));
    // Replacing at the limit is fine.
    manifest
        .with_member("u00000", BucketMember::first("j", None).unwrap())
        .unwrap();
    // An empty uuid, a commit that is not a manifest, a bad id, a huge term.
    let member = BucketMember::first("i", None).unwrap();
    assert!(
        BucketManifest::empty("b")
            .unwrap()
            .with_member("", member)
            .is_err()
    );
    assert!(BucketMember::first("i", Some(b"{}".to_vec())).is_err());
    assert!(BucketManifest::empty("Bad Id").is_err());
    assert!(
        BucketManifest::empty("b")
            .unwrap()
            .at_term(u64::MAX)
            .is_err()
    );
}

#[test]
fn every_proper_prefix_is_an_error() {
    let bytes = sample().encode().unwrap();
    for end in 0..bytes.len() {
        assert!(
            BucketManifest::parse(&bytes[..end]).is_err(),
            "prefix {end}"
        );
    }
}

#[test]
fn len_and_is_empty_count_members() {
    let empty = BucketManifest::empty("b").unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    let manifest = sample();
    assert!(!manifest.is_empty());
    assert_eq!(manifest.len(), 2);
    assert_eq!(manifest.bucket(), "b-1");
}
