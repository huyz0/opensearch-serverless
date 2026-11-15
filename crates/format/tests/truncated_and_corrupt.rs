//! Real Java bytes cut short or damaged: every reader refuses them with an
//! error, never a panic or a wrong answer.

// ARITH: sizes and offsets here are fixture lengths and small constants; an overflow
// panics the test in a debug build, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use common::*;
use oss_format::{
    CommitManifest, DescriptorRecord, FileLocation, FormatError, NodePackHeader, Register,
    SegmentBlobHeader, node_pack_format, segment_blob_format,
};

fn header_of(key: &str, end: impl Fn(&[u8]) -> Result<usize, FormatError>) -> Vec<u8> {
    let bytes = blob(key);
    let end = end(&bytes).unwrap();
    bytes[..end].to_vec()
}

#[test]
fn every_prefix_of_a_segment_blob_header_is_truncated() {
    let header = header_of(&segment_blob_key(), segment_blob_format::header_end);
    for cut in 0..header.len() {
        match SegmentBlobHeader::parse(&header[..cut]) {
            Err(FormatError::Truncated { needed, have, .. }) => {
                assert_eq!(have, cut as u64);
                let expected = if cut < segment_blob_format::PREFIX_BYTES {
                    8
                } else {
                    header.len() as u64
                };
                assert_eq!(needed, expected, "at {cut}");
            }
            other => panic!("prefix of {cut} bytes: {other:?}"),
        }
    }
    assert!(SegmentBlobHeader::parse(&header).is_ok());
}

#[test]
fn every_prefix_of_a_node_pack_header_is_truncated() {
    let header = header_of(&pack_key(), node_pack_format::header_end);
    for cut in 0..header.len() {
        match NodePackHeader::parse(&header[..cut]) {
            Err(FormatError::Truncated { needed, .. }) => {
                let expected = if cut < node_pack_format::PREFIX_BYTES {
                    9
                } else {
                    header.len() as u64
                };
                assert_eq!(needed, expected, "at {cut}");
            }
            other => panic!("prefix of {cut} bytes: {other:?}"),
        }
    }
}

#[test]
fn every_prefix_of_a_manifest_or_descriptor_is_refused() {
    for key in [manifest_key(), "indices/fixture".to_owned()] {
        let bytes = blob(&key);
        for cut in 0..bytes.len() {
            let Ok(register) = Register::parse(&bytes[..cut]) else {
                assert!(cut < 8, "{key}: a register of {cut} bytes");
                continue;
            };
            assert!(
                CommitManifest::parse(register.value).is_err(),
                "{key} cut at {cut}"
            );
            assert!(
                DescriptorRecord::parse(register.value).is_err(),
                "{key} cut at {cut}"
            );
        }
    }
}

/// Flipping any one bit of a header either is refused or still yields a
/// header whose ranges stay inside what it claims; it never panics.
#[test]
fn single_bit_flips_never_panic() {
    let segment = header_of(&segment_blob_key(), segment_blob_format::header_end);
    let pack = header_of(&pack_key(), node_pack_format::header_end);
    let manifest = blob(&manifest_key());
    let descriptor = blob("indices/fixture");
    for (name, bytes) in [
        ("segment", &segment),
        ("pack", &pack),
        ("manifest", &manifest),
        ("descriptor", &descriptor),
    ] {
        for at in 0..bytes.len() {
            for bit in 0..8 {
                let mut damaged = bytes.clone();
                damaged[at] ^= 1 << bit;
                match name {
                    "segment" => {
                        if let Ok(h) = SegmentBlobHeader::parse(&damaged) {
                            assert!(h.files().iter().all(|f| f.offset >= h.header_end()));
                        }
                    }
                    "pack" => {
                        if let Ok(h) = NodePackHeader::parse(&damaged) {
                            assert!(h.blocks().iter().all(|b| b.offset >= h.body_start));
                        }
                    }
                    _ => {
                        let value = &damaged[8..];
                        let _ = CommitManifest::parse(value);
                        let _ = DescriptorRecord::parse(value);
                    }
                }
            }
        }
    }
}

fn pack_with(edit: impl Fn(&mut Vec<u8>)) -> Result<(), FormatError> {
    let mut header = header_of(&pack_key(), node_pack_format::header_end);
    edit(&mut header);
    NodePackHeader::parse(&header).map(|_| ())
}

#[test]
fn node_pack_prefix_damage_is_named() {
    let corrupt = |r: Result<(), FormatError>| matches!(r, Err(FormatError::Corrupt { .. }));
    assert!(corrupt(pack_with(|h| h[0] = b'X')), "magic");
    assert_eq!(
        pack_with(|h| h[4] = 3),
        Err(FormatError::Unsupported {
            what: "node pack header",
            version: 3
        }),
        "version"
    );
    assert!(
        corrupt(pack_with(|h| h[5] = 0x80)),
        "negative header length"
    );
    // A header longer than its length says leaves bytes over; shorter runs a field past it.
    assert!(corrupt(pack_with(|h| {
        let n = u32::from_be_bytes(h[5..9].try_into().unwrap()) + 1;
        h[5..9].copy_from_slice(&n.to_be_bytes());
        h.push(0);
    })));
    assert!(corrupt(pack_with(|h| {
        let n = u32::from_be_bytes(h[5..9].try_into().unwrap()) - 1;
        h[5..9].copy_from_slice(&n.to_be_bytes());
        h.pop();
    })));
}

/// A one-block pack, version 2, with the given uuid and blob name.
fn one_block_pack(uuid: &str, name: &str) -> Result<(), FormatError> {
    pack_of(&[(uuid, name, None, 5)])
}

/// A version 2 pack of blocks (uuid, blob name, offset or right after the
/// one before, length).
fn pack_of(blocks: &[(&str, &str, Option<i64>, i64)]) -> Result<(), FormatError> {
    let utf = |out: &mut Vec<u8>, s: &str| {
        out.extend_from_slice(&u16::try_from(s.len()).unwrap().to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    };
    let mut body = Vec::new();
    utf(&mut body, "n");
    body.extend_from_slice(&0i64.to_be_bytes());
    body.extend_from_slice(&0i64.to_be_bytes());
    body.extend_from_slice(&i32::try_from(blocks.len()).unwrap().to_be_bytes());
    let mut ends = Vec::new();
    for (uuid, name, _, _) in blocks {
        utf(&mut body, uuid);
        utf(&mut body, "index");
        body.extend_from_slice(&0i32.to_be_bytes());
        utf(&mut body, name);
        ends.push(body.len());
        body.extend_from_slice(&[0; 16]);
    }
    let body_start = 9 + body.len() as i64;
    let mut next = body_start;
    for ((_, _, offset, length), end) in blocks.iter().zip(ends) {
        let offset = offset.map_or(next, |o| body_start + o);
        body[end..end + 8].copy_from_slice(&offset.to_be_bytes());
        body[end + 8..end + 16].copy_from_slice(&length.to_be_bytes());
        next = offset + length;
    }
    let mut pack = node_pack_format::MAGIC.to_be_bytes().to_vec();
    pack.push(node_pack_format::VERSION);
    pack.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    pack.extend_from_slice(&body);
    NodePackHeader::parse(&pack).map(|_| ())
}

#[test]
fn a_pack_block_names_its_index_and_blob() {
    assert_eq!(one_block_pack("u", "_0.seg"), Ok(()));
    assert!(matches!(
        one_block_pack("", "_0.seg"),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        one_block_pack("u", ""),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn version_one_packs_carry_no_index_name() {
    // Rewrite the fixture's version 2 header as version 1: drop the index name.
    let header = header_of(&pack_key(), node_pack_format::header_end);
    let parsed = NodePackHeader::parse(&header).unwrap();
    let block = &parsed.blocks()[0];
    let mut body = Vec::new();
    let utf = |out: &mut Vec<u8>, s: &str| {
        out.extend_from_slice(&u16::try_from(s.len()).unwrap().to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    };
    utf(&mut body, &parsed.node);
    body.extend_from_slice(&(parsed.epoch as i64).to_be_bytes());
    body.extend_from_slice(&(parsed.ordinal as i64).to_be_bytes());
    body.extend_from_slice(&1i32.to_be_bytes());
    utf(&mut body, &block.index_uuid);
    body.extend_from_slice(&0i32.to_be_bytes());
    utf(&mut body, &block.name);
    let body_start = 9 + body.len() as u64 + 16;
    body.extend_from_slice(&(body_start as i64).to_be_bytes());
    body.extend_from_slice(&(block.length as i64).to_be_bytes());
    let mut v1 = Vec::new();
    v1.extend_from_slice(&node_pack_format::MAGIC.to_be_bytes());
    v1.push(node_pack_format::VERSION_UUID_ONLY);
    v1.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    v1.extend_from_slice(&body);
    let old = NodePackHeader::parse(&v1).unwrap();
    assert_eq!(old.version, 1);
    assert_eq!(old.blocks()[0].index_name, "");
    assert_eq!(old.blocks()[0].index_uuid, UUID);
    assert_eq!(old.blocks()[0].offset, old.body_start);
}

fn manifest_value(json: &str) -> Result<CommitManifest<'_>, FormatError> {
    CommitManifest::parse(json.as_bytes())
}

#[test]
fn manifest_damage_is_refused() {
    let ok = r#"{"term":1,"files":{"_0.si":"t=1/_0.seg#8:10"},"lengths":{"_0.si":10}}"#;
    assert!(manifest_value(ok).is_ok());
    for bad in [
        r#"[]"#,
        r#"{"files":{}}"#,
        r#"{"term":1}"#,
        r#"{"term":-1,"files":{}}"#,
        r#"{"term":"1","files":{}}"#,
        r#"{"term":1.5,"files":{}}"#,
        r#"{"term":1,"files":[]}"#,
        r#"{"term":1,"files":{"_0.si":""}}"#,
        r#"{"term":1,"files":{"_0.si":7}}"#,
        r#"{"term":1,"files":{"_0.si":"t=1/#8:10"}}"#,
        r#"{"term":1,"files":{"_0.si":"t=1/_0.seg#8:10","_0.si":"t=1/_0.seg#8:10"}}"#,
        r#"{"term":1,"files":{"_0.si":"t=1/_0.seg#8:10"},"lengths":{"_0.si":11}}"#,
        r#"{"term":1,"files":{},"lengths":{"_0.si":-1}}"#,
        r#"{"term":1,"files":{},"lengths":[]}"#,
        r#"{"term":1,"files":{},"lengths":{"a":"1"}}"#,
        r#"{"term":1,"files":{"a":"t=1"},"lengths":{"a":1,"a":2}}"#,
        r#"{"term":1,"files":{},"digest":{"f":"\q"}}"#,
        r#"{"term":1,"files":{},"writer":3}"#,
        r#"{"term":1,"files":{},"digest":[]}"#,
        r#"{"term":1,"files":{}} {}"#,
        r#"{"term":1,"files":{},}"#,
    ] {
        assert!(
            matches!(manifest_value(bad), Err(FormatError::Corrupt { .. })),
            "{bad}"
        );
    }
    // Unknown fields of any shape are skipped; null writer is no writer; a
    // length for a file not named is ignored.
    let tolerant = r#"{"x":{"y":[1,{"z":null}]},"term":0,"writer":null,"files":{},"lengths":{"gone":3},"later":true}"#;
    let m = manifest_value(tolerant).unwrap();
    assert_eq!(
        (
            m.term,
            m.writer.clone(),
            m.files().len(),
            m.wal_ordinal,
            m.stage_sequence
        ),
        (0, None, 0, -1, -1)
    );
    // A whole-blob file takes its length from `lengths`, or has none.
    let whole =
        manifest_value(r#"{"term":3,"files":{"a":"t=3","b":"t=3"},"lengths":{"a":5}}"#).unwrap();
    assert_eq!(whole.file("a").unwrap().length, Some(5));
    assert_eq!(whole.file("b").unwrap().length, None);
}

#[test]
fn locations_follow_the_java_rules() {
    let parse = FileLocation::parse;
    assert_eq!(parse("t=4"), Ok(FileLocation::Whole { term_dir: "t=4" }));
    assert_eq!(
        parse("t=4/a/b.seg#+07:0"),
        Ok(FileLocation::Packed {
            term_dir: "t=4",
            blob: "a/b.seg",
            offset: 7,
            length: 0
        })
    );
    // The largest Java long is the largest offset.
    assert_eq!(
        parse("t=1/x#9223372036854775807:0"),
        Ok(FileLocation::Packed {
            term_dir: "t=1",
            blob: "x",
            offset: i64::MAX as u64,
            length: 0
        })
    );
    let node = format!("@n1/e={:020}/{:020}#12:34", 5, 6);
    assert_eq!(
        parse(&node),
        Ok(FileLocation::NodePacked {
            node: "n1",
            epoch: 5,
            ordinal: 6,
            offset: 12,
            length: 34
        })
    );
    for bad in [
        "",
        "/x#1:2",
        "t=1/x",
        "t=1/x#1",
        "t=1/x#:2",
        "t=1/x#1:",
        "t=1/x#-1:2",
        "t=1/x#1:9223372036854775808",
        "t=1/x#1:a",
        "t=1/x#1:++2",
        "t=1/x#1:18446744073709551616",
        "@",
        "@n1",
        "@n1#1:2",
        "@/e=00000000000000000005/00000000000000000006#1:2",
        "@n1/e=5/00000000000000000006#1:2",
        "@n1/x=00000000000000000005/00000000000000000006#1:2",
        "@n1/e=00000000000000000005/00000000000000000006#01:2",
        "@n1/e=00000000000000000005/00000000000000000006#+1:2",
        "@n1/e=00000000000000000005/00000000000000000006#1",
        "@n1/e=00000000000000000005/00000000000000000006",
        "@n1/e=00000000000000000005/x/00000000000000000006#1:2",
        "@n1/e=00000000000000000005#1:2",
    ] {
        assert!(
            matches!(parse(bad), Err(FormatError::Corrupt { .. })),
            "{bad}"
        );
    }
}

#[test]
fn descriptor_records_are_told_apart() {
    assert_eq!(
        DescriptorRecord::parse(br#"{"tombstone":true,"deleted_at":1}"#),
        Ok(DescriptorRecord::Tombstone)
    );
    assert_eq!(
        DescriptorRecord::parse(br#"{"alias":"a","indices":["x"]}"#),
        Ok(DescriptorRecord::Alias)
    );
    let bucketed =
        br#"{"name":"b","uuid":"u","number_of_shards":2,"bucket":"k1","unit_layout":{"units":4}}"#;
    let DescriptorRecord::Index(d) = DescriptorRecord::parse(bucketed).unwrap() else {
        panic!()
    };
    assert_eq!(
        (d.bucket.as_deref(), d.has_unit_layout, d.number_of_shards),
        (Some("k1"), true, 2)
    );
    for bad in [
        &br#"{"uuid":"u","number_of_shards":1}"#[..],
        br#"{"name":"n","number_of_shards":1}"#,
        br#"{"name":"n","uuid":"u"}"#,
        br#"{"name":"n","uuid":"u","number_of_shards":0}"#,
        br#"{"name":"n","uuid":"u","number_of_shards":2147483648}"#,
        br#"{"name":1,"uuid":"u","number_of_shards":1}"#,
        br#"{"name":"n","uuid":"u","number_of_shards":"1"}"#,
        br#""x""#,
        b"",
        b"{\"name\":\"\xff\",\"uuid\":\"u\",\"number_of_shards\":1}",
    ] {
        assert!(
            matches!(
                DescriptorRecord::parse(bad),
                Err(FormatError::Corrupt { .. })
            ),
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn segment_blob_damage_is_refused() {
    let with = |json: &str| {
        let mut blob = b"SEG1".to_vec();
        blob.extend_from_slice(&u32::try_from(json.len()).unwrap().to_be_bytes());
        blob.extend_from_slice(json.as_bytes());
        SegmentBlobHeader::parse(&blob).map(|h| h.files().len())
    };
    // Offsets count the header: 8 bytes plus this JSON's 45.
    assert_eq!(
        with(r#"{"version":1,"files":{"a":[53,3],"b":[56,0]}}"#),
        Ok(2)
    );
    // A gap between files is allowed; the Java writes none, but nothing reads one.
    assert_eq!(
        with(r#"{"version":1,"files":{"a":[99,3],"b":[200,0]}}"#),
        Ok(2)
    );
    assert_eq!(
        with(r#"{"version":2,"files":{}}"#),
        Err(FormatError::Unsupported {
            what: "segment blob header",
            version: 2
        })
    );
    for bad in [
        r#"{"files":{}}"#,
        r#"{"version":1}"#,
        r#"{"version":"1","files":{}}"#,
        r#"{"version":1,"files":[]}"#,
        r#"{"version":1,"files":{"a":[1,3]}}"#,
        r#"{"version":1,"files":{"a":[100,3],"b":[101,1]}}"#,
        r#"{"version":1,"files":{"a":[41,3]}}"#,
        r#"{"version":1,"files":{"a":[40,-1]}}"#,
        r#"{"version":1,"files":{"a":[40]}}"#,
        r#"{"version":1,"files":{"a":[40,1,2]}}"#,
        r#"{"version":1,"files":{"a":40}}"#,
        r#"{"version":1,"files":{"a":[40,1],"a":[41,1]}}"#,
    ] {
        assert!(
            matches!(with(bad), Err(FormatError::Corrupt { .. })),
            "{bad}"
        );
    }
    // The widest range a Java long can name still ends inside a u64.
    assert_eq!(
        with(r#"{"version":1,"files":{"a":[9223372036854775807,1]}}"#),
        Ok(1)
    );
    let mut wrong = blob(&segment_blob_key());
    wrong[3] = b'2';
    assert!(matches!(
        segment_blob_format::header_end(&wrong),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn records_a_reader_cannot_follow_say_so() {
    let not_followable =
        |r: Result<(), FormatError>| matches!(r, Err(FormatError::NotFollowable { .. }));
    assert!(not_followable(
        DescriptorRecord::Alias.into_index().map(|_| ())
    ));
    assert!(not_followable(
        DescriptorRecord::Tombstone.into_index().map(|_| ())
    ));
    let bucketed = br#"{"name":"b","uuid":"u","number_of_shards":2,"bucket":"k1"}"#;
    let index = DescriptorRecord::parse(bucketed)
        .unwrap()
        .into_index()
        .unwrap();
    assert!(not_followable(index.shard(0).map(|_| ())));
    let laid_out = br#"{"name":"b","uuid":"u","number_of_shards":2,"unit_layout":{}}"#;
    let index = DescriptorRecord::parse(laid_out)
        .unwrap()
        .into_index()
        .unwrap();
    assert_eq!(index.shard(9).unwrap().shard, 9);
}
