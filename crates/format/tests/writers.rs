//! The writers of what this crate reads: segment blob headers, file
//! locations and commit manifests, checked against the bytes the Java shell
//! wrote (fixtures/shell-blobs) and against this crate's own readers.

// ARITH: sizes and offsets here are fixture lengths and small constants; an overflow
// panics the test in a debug build, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use common::*;
use oss_format::segment_blob_format::PREFIX_BYTES;
use oss_format::{
    CommitManifest, FileLocation, ManifestEntry, Register, SegmentBlobHeader, encode_manifest,
    encode_segment_header, segment_blob_order,
};

/// AC-1: the Java's header, re-encoded from its parsed files.
#[test]
fn the_javas_segment_header_is_re_encoded_byte_for_byte() {
    let blob = blob(&segment_blob_key());
    let header = SegmentBlobHeader::parse(&blob).unwrap();
    let end = usize::try_from(header.header_end()).unwrap();
    let files: Vec<(&str, u64)> = header
        .files()
        .iter()
        .map(|f| (f.name.as_ref(), f.length))
        .collect();
    assert_eq!(encode_segment_header(&files).unwrap(), blob[..end]);
}

#[test]
fn a_header_is_settled_when_the_offsets_gain_a_digit() {
    // Offsets begin at the header's own length, so a file count that pushes
    // an offset from three digits to four moves the header's length too.
    for count in [1usize, 2, 3, 7, 40, 200, 1_000] {
        let names: Vec<String> = (0..count).map(|n| format!("_0_{n:04}.x")).collect();
        let files: Vec<(&str, u64)> = names.iter().map(|n| (n.as_str(), 90_000)).collect();
        let bytes = encode_segment_header(&files).unwrap();
        let parsed = SegmentBlobHeader::parse(&bytes).unwrap();
        assert_eq!(usize::try_from(parsed.header_end()).unwrap(), bytes.len());
        assert_eq!(parsed.files().len(), count);
        assert_eq!(parsed.files()[0].offset, bytes.len() as u64, "{count}");
        for pair in parsed.files().windows(2) {
            assert_eq!(pair[1].offset, pair[0].offset + pair[0].length);
        }
    }
}

#[test]
fn a_header_refuses_what_a_blob_cannot_hold() {
    assert!(encode_segment_header(&[]).is_err(), "no files");
    assert!(
        encode_segment_header(&[("a", 1), ("a", 2)]).is_err(),
        "a name twice"
    );
    let many: Vec<String> = (0..=1_024).map(|n| format!("f{n}")).collect();
    let files: Vec<(&str, u64)> = many.iter().map(|n| (n.as_str(), 1)).collect();
    assert!(encode_segment_header(&files).is_err(), "more than 1,024");
    let at_limit: Vec<(&str, u64)> = files.iter().take(1_024).copied().collect();
    assert!(encode_segment_header(&at_limit).is_ok());
}

#[test]
fn files_go_in_a_blob_smallest_first_then_by_name() {
    let mut files = vec![("b", 5), ("a", 5), ("z", 1), ("c", 100)];
    segment_blob_order(&mut files);
    assert_eq!(files, [("z", 1), ("a", 5), ("b", 5), ("c", 100)]);
    // The Java's header for the same files is what is written.
    let bytes = encode_segment_header(&files).unwrap();
    let header = SegmentBlobHeader::parse(&bytes).unwrap();
    let names: Vec<&str> = header.files().iter().map(|f| f.name.as_ref()).collect();
    assert_eq!(names, ["z", "a", "b", "c"]);
}

/// AC-2: every location form encodes to the text it parses from.
#[test]
fn locations_encode_to_the_text_they_parse_from() {
    for text in [
        "t=2",
        "t=2/_1.seg#445:397",
        "t=17/_5-AbC_d-9.seg#0:0",
        "@tZ9iGQAAQACwLK7Z_____w/e=00000001794710559389/00000000000000000000#538:333",
    ] {
        assert_eq!(FileLocation::parse(text).unwrap().encode(), text);
    }
}

/// AC-2: the Java's manifest, re-encoded, parses to the same commit.
#[test]
fn the_javas_manifest_re_encodes_to_the_same_commit() {
    let bytes = blob(&manifest_key());
    let register = Register::parse(&bytes).unwrap();
    let parsed = CommitManifest::parse(register.value).unwrap();
    let entries: Vec<ManifestEntry<'_>> = parsed
        .files()
        .iter()
        .map(|f| ManifestEntry {
            name: f.name.as_ref(),
            location: f.location.encode(),
            length: f.length.unwrap(),
        })
        .collect();
    let encoded = encode_manifest(
        parsed.term,
        parsed.writer.as_deref(),
        &entries,
        parsed.wal_ordinal,
        parsed.stage_sequence,
    )
    .unwrap();
    let again = CommitManifest::parse(&encoded).unwrap();
    assert_eq!(again.term, parsed.term);
    assert_eq!(again.writer, parsed.writer);
    assert_eq!(again.wal_ordinal, parsed.wal_ordinal);
    assert_eq!(again.stage_sequence, parsed.stage_sequence);
    assert_eq!(again.files(), parsed.files());
}

#[test]
fn a_manifest_is_the_javas_json_in_the_javas_order() {
    let entries = [
        ManifestEntry {
            name: "segments_3",
            location: "t=4/_1.seg#100:20".to_owned(),
            length: 20,
        },
        ManifestEntry {
            name: "_0.si",
            location: "t=4".to_owned(),
            length: 7,
        },
    ];
    let bytes = encode_manifest(4, Some("node-a"), &entries, 9, -1).unwrap();
    // Names sorted; `wal_ordinal` after the lengths; no `stage_seq` when unset.
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        r#"{"term":4,"writer":"node-a","files":{"_0.si":"t=4","segments_3":"t=4/_1.seg#100:20"},"lengths":{"_0.si":7,"segments_3":20},"wal_ordinal":9}"#
    );
    let no_writer = encode_manifest(4, None, &entries, -1, 2).unwrap();
    assert_eq!(
        std::str::from_utf8(&no_writer).unwrap(),
        r#"{"term":4,"files":{"_0.si":"t=4","segments_3":"t=4/_1.seg#100:20"},"lengths":{"_0.si":7,"segments_3":20},"stage_seq":2}"#
    );
}

#[test]
fn a_manifest_escapes_names_and_refuses_what_it_cannot_hold() {
    let odd = [ManifestEntry {
        name: "a\"b\\c\n",
        location: "t=1".to_owned(),
        length: 1,
    }];
    let bytes = encode_manifest(1, None, &odd, -1, -1).unwrap();
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains(r#""a\"b\\c\n""#)
    );
    assert!(encode_manifest(1, None, &odd, -1, -1).is_ok());
    let twice = [
        ManifestEntry {
            name: "x",
            location: "t=1".to_owned(),
            length: 1,
        },
        ManifestEntry {
            name: "x",
            location: "t=1".to_owned(),
            length: 1,
        },
    ];
    assert!(
        encode_manifest(1, None, &twice, -1, -1).is_err(),
        "a name twice"
    );
    let bad_location = [ManifestEntry {
        name: "x",
        location: "t=1/blob".to_owned(),
        length: 1,
    }];
    assert!(
        encode_manifest(1, None, &bad_location, -1, -1).is_err(),
        "a location no reader follows"
    );
    let many: Vec<ManifestEntry<'static>> = (0..=16_384)
        .map(|_| ManifestEntry {
            name: "x",
            location: "t=1".to_owned(),
            length: 1,
        })
        .collect();
    assert!(
        encode_manifest(1, None, &many, -1, -1).is_err(),
        "more than 16,384 files"
    );
}

#[test]
fn the_prefix_is_the_magic_and_a_big_endian_length() {
    let bytes = encode_segment_header(&[("_0.si", 3)]).unwrap();
    assert_eq!(&bytes[..4], b"SEG1");
    let len = u32::from_be_bytes(bytes[4..PREFIX_BYTES].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), PREFIX_BYTES + len);
}
