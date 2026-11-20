//! docs/specs/bucket-manifest-reader/SPEC.md, AC-6 and AC-8: bytes that are not a manifest.

mod bucket_support;

use bucket_support::{DEFLATE, LZ4, NONE, ZSTD, compact, frame, lz4, sample_commit, string, vlong};
use bucket_support::{MAGIC, Member};
use oss_format::{FormatError, parse_bucket_member};

fn members() -> Vec<Member> {
    let mut commit = sample_commit();
    commit.files.retain(|f| f.name != "write.lock");
    vec![
        Member {
            uuid: "aaaa".into(),
            index: "alpha".into(),
            version: 2,
            commit: Some(commit),
        },
        Member {
            uuid: "zzzz".into(),
            index: "empty".into(),
            version: 0,
            commit: None,
        },
    ]
}

fn refused(bytes: &[u8]) -> FormatError {
    parse_bucket_member(bytes, "aaaa").unwrap_err()
}

#[test]
fn no_proper_prefix_of_a_compact_manifest_is_read_or_panics() {
    for codec in [NONE, DEFLATE, ZSTD, LZ4] {
        let bytes = compact(codec, "b-1", 5, &members());
        assert!(parse_bucket_member(&bytes, "aaaa").is_ok());
        for end in 0..bytes.len() {
            // A cut body is refused; a cut header says how much it needs.
            let result = parse_bucket_member(&bytes[..end], "aaaa");
            assert!(result.is_err(), "codec {codec} cut at {end}");
        }
    }
}

#[test]
fn a_short_header_says_how_many_bytes_it_needs() {
    assert!(matches!(
        refused(&MAGIC[..]),
        FormatError::Truncated {
            needed: 10,
            have: 4,
            ..
        }
    ));
}

#[test]
fn an_unknown_format_or_codec_is_unsupported() {
    let mut bytes = compact(NONE, "b-1", 5, &members());
    bytes[4] = 3;
    assert!(matches!(
        refused(&bytes),
        FormatError::Unsupported { version: 3, .. }
    ));
    let mut bytes = compact(NONE, "b-1", 5, &members());
    bytes[5] = 9;
    assert!(matches!(
        refused(&bytes),
        FormatError::Unsupported { version: 9, .. }
    ));
}

#[test]
fn a_header_length_that_lies_or_is_past_the_cap_is_refused() {
    for codec in [DEFLATE, ZSTD, LZ4] {
        let good = compact(codec, "b-1", 5, &members());
        for delta in [-1i64, 1] {
            let mut bytes = good.clone();
            let claimed = i64::from(u32::from_be_bytes(bytes[6..10].try_into().unwrap())) + delta;
            bytes[6..10].copy_from_slice(&u32::try_from(claimed).unwrap().to_be_bytes());
            assert!(
                parse_bucket_member(&bytes, "aaaa").is_err(),
                "codec {codec} claiming {delta:+}"
            );
        }
    }
    let mut over = compact(NONE, "b", 1, &members());
    over[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(refused(&over), FormatError::TooLarge { limit, .. } if limit == 64 << 20));
    // A zstd bomb: the header claims 64 MiB and the stream holds a few bytes.
    let at_cap = u32::try_from(64usize << 20).unwrap();
    let mut bomb = compact(ZSTD, "b", 1, &members());
    bomb[6..10].copy_from_slice(&at_cap.to_be_bytes());
    assert!(parse_bucket_member(&bomb, "aaaa").is_err());
}

fn raw_body(strings: &[&str], members: &[(&str, Vec<u8>)], bucket: &str) -> Vec<u8> {
    let mut out = Vec::new();
    string(&mut out, bucket);
    vlong(&mut out, 1);
    vlong(&mut out, strings.len() as u64);
    for s in strings {
        string(&mut out, s);
    }
    vlong(&mut out, members.len() as u64);
    for (uuid, entry) in members {
        string(&mut out, uuid);
        vlong(&mut out, entry.len() as u64);
    }
    for (_, entry) in members {
        out.extend(entry);
    }
    out
}

fn entry(version: u64, commit: Option<&[u8]>) -> Vec<u8> {
    let mut e = Vec::new();
    string(&mut e, "idx");
    vlong(&mut e, version);
    e.push(u8::from(commit.is_some()));
    if let Some(c) = commit {
        e.extend(c);
    }
    e
}

/// A commit with no packs and one file whose stem and extension are strings 0 and 1.
fn commit_with_file(stem: u64, tag: u64, location: u64, trailing: &[u8]) -> Vec<u8> {
    let mut c = Vec::new();
    for v in [1u64, 0, 0, 0, 0] {
        vlong(&mut c, v);
    }
    vlong(&mut c, 1);
    for v in [stem, 1, tag, location] {
        vlong(&mut c, v);
    }
    c.extend(trailing);
    c
}

fn read(raw: Vec<u8>) -> Result<(), FormatError> {
    let bytes = frame(NONE, raw.len(), &raw);
    parse_bucket_member(&bytes, "u").map(drop)
}

#[test]
fn a_well_formed_hand_built_body_reads() {
    let commit = commit_with_file(0, 0, 2, &[0, 0]);
    let raw = raw_body(
        &["_0", ".si", "t=1"],
        &[("u", entry(1, Some(&commit)))],
        "b",
    );
    read(raw).unwrap();
}

#[test]
fn a_string_index_past_the_table_is_refused() {
    let commit = commit_with_file(7, 0, 2, &[0, 0]);
    let raw = raw_body(
        &["_0", ".si", "t=1"],
        &[("u", entry(1, Some(&commit)))],
        "b",
    );
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
}

#[test]
fn a_pack_that_is_not_listed_and_unknown_tags_are_refused() {
    // Location tag 1 is a range in a pack, and there are no packs.
    let range = commit_with_file(0, 1 << 2, 0, &[0, 0, 0, 0]);
    let raw = raw_body(&["_0", ".si", "t=1"], &[("u", entry(1, Some(&range)))], "b");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
    // Length tag 1 on a location that has no range, and length tag 3.
    for tag in [1u64, 3] {
        let commit = commit_with_file(0, tag, 2, &[0, 0]);
        let raw = raw_body(
            &["_0", ".si", "t=1"],
            &[("u", entry(1, Some(&commit)))],
            "b",
        );
        assert!(
            matches!(read(raw), Err(FormatError::Corrupt { .. })),
            "length tag {tag}"
        );
    }
}

#[test]
fn a_version_and_commit_that_disagree_are_refused() {
    let commit = commit_with_file(0, 0, 2, &[0, 0]);
    let with = raw_body(
        &["_0", ".si", "t=1"],
        &[("u", entry(0, Some(&commit)))],
        "b",
    );
    assert!(matches!(read(with), Err(FormatError::Corrupt { .. })));
    let without = raw_body(&["_0"], &[("u", entry(3, None))], "b");
    assert!(matches!(read(without), Err(FormatError::Corrupt { .. })));
}

#[test]
fn an_entry_that_is_not_its_recorded_length_or_has_a_bad_flag_is_refused() {
    let commit = commit_with_file(0, 0, 2, &[0, 0]);
    let mut long = entry(1, Some(&commit));
    long.push(0);
    let raw = raw_body(&["_0", ".si", "t=1"], &[("u", long)], "b");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
    let mut flag = entry(1, Some(&commit));
    flag[5] = 2;
    let raw = raw_body(&["_0", ".si", "t=1"], &[("u", flag)], "b");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
}

#[test]
fn counts_past_the_bytes_left_are_refused_before_anything_is_sized_by_them() {
    let mut raw = Vec::new();
    string(&mut raw, "b");
    vlong(&mut raw, 1);
    vlong(&mut raw, 2_000_000_000);
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
    let mut members = Vec::new();
    string(&mut members, "b");
    vlong(&mut members, 1);
    vlong(&mut members, 0);
    vlong(&mut members, 2_000_000_000);
    assert!(matches!(read(members), Err(FormatError::Corrupt { .. })));
    // Entries that run past the body.
    let mut past = Vec::new();
    string(&mut past, "b");
    vlong(&mut past, 1);
    vlong(&mut past, 0);
    vlong(&mut past, 1);
    string(&mut past, "u");
    vlong(&mut past, 50);
    assert!(matches!(read(past), Err(FormatError::Corrupt { .. })));
    let mut files = commit_with_file(0, 0, 2, &[0, 0]);
    // A file count past MAX_FILES, in place of the one file.
    files.truncate(5);
    vlong(&mut files, 20_000);
    let raw = raw_body(&["_0", ".si", "t=1"], &[("u", entry(1, Some(&files)))], "b");
    assert!(matches!(read(raw), Err(FormatError::TooLarge { .. })));
}

#[test]
fn a_bad_bucket_id_or_string_is_refused() {
    let raw = raw_body(&[], &[("u", entry(0, None))], "Not Valid");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
    // A lone high surrogate, then a byte that starts no character.
    let mut lone = Vec::new();
    vlong(&mut lone, 1);
    lone.extend([0xED, 0xA0, 0x80]);
    assert!(parse_bucket_member(&frame(NONE, lone.len(), &lone), "u").is_err());
    let mut bad = Vec::new();
    vlong(&mut bad, 1);
    bad.push(0xFF);
    assert!(parse_bucket_member(&frame(NONE, bad.len(), &bad), "u").is_err());
}

#[test]
fn a_variable_length_number_past_its_bytes_is_refused() {
    let mut raw = Vec::new();
    string(&mut raw, "b");
    raw.extend([0xFF; 10]);
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
}

#[test]
fn a_lz4_stream_with_a_zero_or_far_back_offset_is_refused() {
    // One literal, then a match back 0, and back 9 with a single byte written.
    for back in [0u8, 9] {
        let packed = vec![0x10 | 0x01, b'x', back, 0];
        let bytes = frame(LZ4, 20, &packed);
        assert!(parse_bucket_member(&bytes, "u").is_err(), "back {back}");
    }
    assert!(lz4(&[1, 2, 3]).len() >= 4);
}

#[test]
fn json_that_is_not_a_manifest_is_refused() {
    for bytes in [
        &b""[..],
        b"[]",
        b"{}",
        br#"{"bucket":"b"}"#,
        br#"{"bucket":"b","term":-1,"members":{}}"#,
        br#"{"bucket":"B","term":1,"members":{}}"#,
        br#"{"bucket":"b","term":1,"members":{"u":[]}}"#,
        br#"{"bucket":"b","term":1,"members":{"u":{"version":1}}}"#,
        br#"{"bucket":"b","term":1,"members":{"u":{"index":"i","version":-1}}}"#,
        br#"{"bucket":"b","term":1,"members":{"u":{"index":"i","commit":{"term":1}}}}"#,
        br#"{"bucket":"b","term":1,"members":{"u":{"index":"i""#,
    ] {
        assert!(
            parse_bucket_member(bytes, "u").is_err(),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

/// A commit with one node pack of the given deltas and one file in it.
fn commit_in_node_pack(epoch: i64, ordinal: i64) -> Vec<u8> {
    let mut c = Vec::new();
    for v in [1u64, 0, 0, 0] {
        vlong(&mut c, v);
    }
    vlong(&mut c, 1);
    vlong(&mut c, 2 << 1 | 1);
    bucket_support::zlong(&mut c, epoch);
    bucket_support::zlong(&mut c, ordinal);
    vlong(&mut c, 1);
    for v in [0u64, 1, 1 << 2, 0, 0, 5] {
        vlong(&mut c, v);
    }
    c.extend([0, 0]);
    c
}

fn node_pack_commit(epoch: i64, ordinal: i64) -> Result<(), FormatError> {
    let commit = commit_in_node_pack(epoch, ordinal);
    read(raw_body(
        &["_0", ".si", "node"],
        &[("u", entry(1, Some(&commit)))],
        "b",
    ))
}

#[test]
fn a_node_pack_at_epoch_and_ordinal_zero_is_read_and_a_negative_one_is_not() {
    node_pack_commit(0, 0).unwrap();
    assert!(node_pack_commit(-1, 0).is_err());
    assert!(node_pack_commit(0, -1).is_err());
    assert!(node_pack_commit(i64::MAX, 0).is_ok());
}

#[test]
fn a_commit_may_name_exactly_the_most_files_and_one_more_is_refused() {
    for (files, ok) in [(16_384u64, true), (16_385, false)] {
        let mut c = Vec::new();
        for v in [1u64, 0, 0, 0, 0, files] {
            vlong(&mut c, v);
        }
        for _ in 0..files {
            for v in [0u64, 1, 0, 2] {
                vlong(&mut c, v);
            }
        }
        c.extend([0, 0]);
        let raw = raw_body(&["_0", ".si", "t=1"], &[("u", entry(1, Some(&c)))], "b");
        assert_eq!(read(raw).is_ok(), ok, "{files} files");
    }
}

#[test]
fn non_ascii_uuids_names_and_writers_are_skipped_and_read() {
    let mut commit = sample_commit();
    commit.files.retain(|f| f.name != "write.lock");
    commit.writer = Some("w-é-日");
    let members: Vec<Member> = ["é-1", "日-2", "z-3"]
        .iter()
        .map(|u| Member {
            uuid: (*u).into(),
            index: format!("i-{u}"),
            version: 1,
            commit: Some(commit.clone()),
        })
        .collect();
    for codec in [NONE, ZSTD] {
        let bytes = compact(codec, "b", 1, &members);
        for u in ["é-1", "日-2", "z-3"] {
            let member = parse_bucket_member(&bytes, u).unwrap().member.unwrap();
            assert_eq!(member.index_name, format!("i-{u}"));
            let json = String::from_utf8(member.commit.unwrap()).unwrap();
            assert!(json.contains("\"writer\":\"w-é-日\""), "{json}");
        }
    }
}

#[test]
fn json_fields_of_other_shapes_are_skipped_where_the_reader_does_not_want_them() {
    let ok = br#"{"extra":{"a":1},"bucket":"b","term":1,"members":{"u":{"digest":{"x":[1]},"index":"i","note":[1,2]}},"later":[{"k":1}]}"#;
    let member = parse_bucket_member(ok, "u").unwrap().member.unwrap();
    assert_eq!((member.index_name.as_str(), member.version), ("i", 0));
    let not_an_object = br#"{"bucket":"b","term":1,"members":5}"#;
    assert_eq!(
        parse_bucket_member(not_an_object, "u").unwrap().member,
        None
    );
    let string_commit = br#"{"bucket":"b","term":1,"members":{"u":{"index":"i","commit":"x"}}}"#;
    assert_eq!(
        parse_bucket_member(string_commit, "u")
            .unwrap()
            .member
            .unwrap()
            .commit,
        None
    );
}

#[test]
fn a_string_past_the_cap_one_string_everywhere_and_too_many_packs_are_refused() {
    let long = "x".repeat(1025);
    let raw = raw_body(&[&long], &[("u", entry(0, None))], "b");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
    let at_cap = "y".repeat(1024);
    // The cap itself is read: a uuid and a string of 1024 characters.
    let mut c = Vec::new();
    for v in [1u64, 0, 0, 0, 0, 1, 0, 0, 0, 1] {
        vlong(&mut c, v);
    }
    c.extend([0, 0]);
    read(raw_body(&[&at_cap], &[(&at_cap, entry(1, Some(&c)))], "b")).unwrap();
    // 16,384 files each naming a 1,024-character string twice and as a location.
    let mut files = Vec::new();
    for v in [1u64, 0, 0, 0, 0, 16_384] {
        vlong(&mut files, v);
    }
    for _ in 0..16_384 {
        for v in [0u64, 0, 0, 0] {
            vlong(&mut files, v);
        }
    }
    files.extend([0, 0]);
    let raw = raw_body(&[&at_cap], &[("u", entry(1, Some(&files)))], "b");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
    // More packs than a commit can use.
    let mut packs = Vec::new();
    for v in [1u64, 0, 0, 0, 16_385] {
        vlong(&mut packs, v);
    }
    packs.extend(std::iter::repeat_n(0u8, 20_000));
    let raw = raw_body(&["p"], &[("u", entry(1, Some(&packs)))], "b");
    assert!(matches!(read(raw), Err(FormatError::Corrupt { .. })));
}

#[test]
fn a_none_body_that_is_not_its_header_length_is_refused() {
    let mut bytes = compact(NONE, "b-1", 5, &members());
    let claimed = u32::from_be_bytes(bytes[6..10].try_into().unwrap());
    bytes[6..10].copy_from_slice(&(claimed + 1).to_be_bytes());
    assert!(matches!(refused(&bytes), FormatError::Corrupt { .. }));
}

#[test]
fn a_write_log_ordinal_and_stage_of_zero_are_kept_and_the_first_duplicate_wins() {
    let mut commit = sample_commit();
    commit.files.retain(|f| f.name != "write.lock");
    commit.wal_ordinal = 0;
    commit.stage = 0;
    let first = Member {
        uuid: "u".into(),
        index: "first".into(),
        version: 1,
        commit: Some(commit.clone()),
    };
    let second = Member {
        uuid: "u".into(),
        index: "second".into(),
        version: 2,
        commit: Some(commit),
    };
    let bytes = compact(NONE, "b", 1, &[first, second]);
    let member = parse_bucket_member(&bytes, "u").unwrap().member.unwrap();
    assert_eq!(member.index_name, "first");
    let json = String::from_utf8(member.commit.unwrap()).unwrap();
    assert!(json.contains("\"wal_ordinal\":0,\"stage_seq\":0"), "{json}");
}
