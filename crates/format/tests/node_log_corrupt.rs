//! Node-log headers cut, flipped or built field by field to break a rule
//! are refused, and encodings the Java refuses write nothing
//! (docs/specs/node-log-format/SPEC.md, AC-3 and AC-4).

// ARITH: offsets here are fixture sizes; an overflow panics the test in a
// debug build, which fails it.
#![allow(clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use oss_format::node_log_format::{
    FENCE_MAGIC, MAX_HEADER_BYTES, PREFIX_BYTES, RECORDS_MAGIC, VERSION, decode, encode,
    encode_fence, header_end,
};
use oss_format::{FormatError, NodeLogEntry, OrdinalRange, ShardBatch, ShardKey, WalRecord};

fn fixture(name: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/node-log")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2).unwrap(), 16).unwrap())
        .collect()
}

fn first_records_blob() -> Vec<u8> {
    let blobs = fixture("blobs.txt");
    let decoded = fixture("decoded.txt");
    // The case with three slices and a running index of three shards.
    let at = decoded
        .iter()
        .position(|d| d.contains("node=node-7"))
        .unwrap();
    unhex(&blobs[at])
}

#[test]
fn every_cut_of_a_header_asks_for_more_or_is_refused_never_misread() {
    let bytes = first_records_blob();
    let end = header_end(&bytes).unwrap().unwrap();
    for cut in 0..end {
        let leading = &bytes[..cut];
        let probe = header_end(leading).unwrap();
        if cut < PREFIX_BYTES {
            assert_eq!(probe, None, "{cut}");
        } else {
            assert_eq!(probe, Some(end));
        }
        match decode(leading) {
            Err(FormatError::Truncated { needed, .. }) => {
                assert!(
                    needed == end as u64 || needed == PREFIX_BYTES as u64,
                    "{cut}"
                );
            }
            other => panic!("cut at {cut}: {other:?}"),
        }
    }
    // The header alone decodes; the body is not needed.
    assert!(matches!(
        decode(&bytes[..end]),
        Ok(NodeLogEntry::Records(_))
    ));
}

#[test]
fn every_flipped_header_byte_is_read_or_refused_without_panic() {
    let bytes = first_records_blob();
    let end = header_end(&bytes).unwrap().unwrap();
    for at in 0..end {
        for flip in [0x01u8, 0x80, 0xFF] {
            let mut bad = bytes.clone();
            bad[at] ^= flip;
            let _ = header_end(&bad);
            let _ = decode(&bad);
        }
    }
}

fn header_of(bytes: &mut Vec<u8>, header: &[u8]) {
    bytes.extend_from_slice(&RECORDS_MAGIC.to_be_bytes());
    bytes.push(VERSION);
    bytes.extend_from_slice(&(header.len() as i32).to_be_bytes());
    bytes.extend_from_slice(header);
}

/// A header with node `n`, epoch 1, ordinal 5, trim 0, one name `u`/`x`,
/// the slices and index given as raw bytes, and their counts.
/// An index entry as raw fields: name position, shard, runs.
type RawEntry<'a> = (i32, i32, &'a [(i64, i64)]);

fn raw(slices: &[[i64; 7]], index: &[RawEntry<'_>], trailing: &[u8]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(&[0, 1, b'n']);
    for n in [1i64, 5, 0] {
        h.extend_from_slice(&n.to_be_bytes());
    }
    h.extend_from_slice(&1i32.to_be_bytes());
    h.extend_from_slice(&[0, 1, b'u', 0, 1, b'x']);
    h.extend_from_slice(&(slices.len() as i32).to_be_bytes());
    for s in slices {
        h.extend_from_slice(&(s[0] as i32).to_be_bytes());
        h.extend_from_slice(&(s[1] as i32).to_be_bytes());
        h.extend_from_slice(&s[2].to_be_bytes());
        h.extend_from_slice(&s[3].to_be_bytes());
        h.extend_from_slice(&(s[4] as i32).to_be_bytes());
        h.extend_from_slice(&(s[5] as i32).to_be_bytes());
        h.extend_from_slice(&s[6].to_be_bytes());
    }
    h.extend_from_slice(&(index.len() as i32).to_be_bytes());
    for (position, shard, runs) in index {
        h.extend_from_slice(&position.to_be_bytes());
        h.extend_from_slice(&shard.to_be_bytes());
        h.extend_from_slice(&(runs.len() as i32).to_be_bytes());
        for (from, to) in *runs {
            h.extend_from_slice(&from.to_be_bytes());
            h.extend_from_slice(&to.to_be_bytes());
        }
    }
    h.extend_from_slice(trailing);
    let mut bytes = Vec::new();
    header_of(&mut bytes, &h);
    bytes
}

fn refused(bytes: &[u8]) -> FormatError {
    decode(bytes).expect_err("decoded")
}

#[test]
fn a_well_formed_raw_header_decodes() {
    let ok = raw(
        &[[0, 0, 1, 0, 10, 1, 3], [0, 1, 1, 10, 5, 1, -2]],
        &[(0, 0, &[(0, 2), (4, 5)])],
        &[],
    );
    let NodeLogEntry::Records(h) = decode(&ok).unwrap() else {
        panic!()
    };
    assert_eq!(h.slices.len(), 2);
    assert_eq!(h.body_length(), 15);
    assert_eq!(h.slices[0].shard.index_name.as_deref(), Some("x"));
}

#[test]
fn a_header_breaking_a_rule_is_refused() {
    let corrupt = |bytes: Vec<u8>| assert!(matches!(refused(&bytes), FormatError::Corrupt { .. }));
    // Slices that overlap or go back.
    corrupt(raw(
        &[[0, 0, 1, 0, 10, 1, 0], [0, 1, 1, 9, 5, 1, 0]],
        &[],
        &[],
    ));
    // A negative offset, length, record count or shard.
    corrupt(raw(&[[0, 0, 1, -1, 10, 1, 0]], &[], &[]));
    corrupt(raw(&[[0, 0, 1, 0, -1, 1, 0]], &[], &[]));
    corrupt(raw(&[[0, 0, 1, 0, 1, -1, 0]], &[], &[]));
    corrupt(raw(&[[0, -1, 1, 0, 1, 1, 0]], &[], &[]));
    // A name position past the names, or negative.
    corrupt(raw(&[[1, 0, 1, 0, 1, 1, 0]], &[], &[]));
    corrupt(raw(&[], &[(-1, 0, &[(0, 0)])], &[]));
    // Runs that are empty, overlap, touch the run before or go back.
    corrupt(raw(&[], &[(0, 0, &[(3, 2)])], &[]));
    corrupt(raw(&[], &[(0, 0, &[(0, 2), (2, 3)])], &[]));
    corrupt(raw(&[], &[(0, 0, &[(3, 4), (0, 1)])], &[]));
    corrupt(raw(&[], &[(0, 0, &[(-1, 1)])], &[]));
    // A shard indexed twice.
    corrupt(raw(&[], &[(0, 0, &[(0, 0)]), (0, 0, &[(1, 1)])], &[]));
    // Bytes left over in the header.
    corrupt(raw(&[], &[], &[0]));
    // A count with more entries than the header has bytes for.
    let mut bytes = raw(&[], &[], &[]);
    let at = bytes.len() - 8;
    bytes[at..at + 4].copy_from_slice(&1000i32.to_be_bytes());
    assert!(matches!(refused(&bytes), FormatError::TooLarge { .. }));
}

#[test]
fn an_index_named_twice_is_refused() {
    let mut bytes = raw(&[], &[], &[]);
    // Name the same uuid twice: count 2, then `u`/`x` again.
    let names = PREFIX_BYTES + 3 + 24;
    bytes[names..names + 4].copy_from_slice(&2i32.to_be_bytes());
    bytes.splice(names + 10..names + 10, [0, 1, b'u', 0, 0]);
    let length = (bytes.len() - PREFIX_BYTES) as i32;
    bytes[5..9].copy_from_slice(&length.to_be_bytes());
    assert_eq!(
        refused(&bytes),
        FormatError::Corrupt {
            what: "node-log blob",
            reason: "an index is named twice"
        }
    );
    // A second, different uuid is fine.
    bytes[names + 12] = b'v';
    assert!(decode(&bytes).is_ok());
}

#[test]
fn a_trim_point_past_the_ordinal_or_a_negative_epoch_is_refused() {
    let mut bytes = raw(&[], &[], &[]);
    // Node id is 3 bytes from the header's start; epoch, ordinal, trim follow.
    let epoch = PREFIX_BYTES + 3;
    bytes[epoch + 16..epoch + 24].copy_from_slice(&6i64.to_be_bytes());
    assert!(matches!(refused(&bytes), FormatError::Corrupt { .. }));
    bytes[epoch + 16..epoch + 24].copy_from_slice(&5i64.to_be_bytes());
    assert!(decode(&bytes).is_ok());
    bytes[epoch..epoch + 8].copy_from_slice(&(-1i64).to_be_bytes());
    assert!(matches!(refused(&bytes), FormatError::Corrupt { .. }));
}

#[test]
fn a_prefix_of_another_kind_or_version_or_length_is_refused() {
    let good = raw(&[], &[], &[]);
    let mut magic = good.clone();
    magic[0] = 0;
    assert!(matches!(
        header_end(&magic),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(refused(&magic), FormatError::Corrupt { .. }));
    let mut version = good.clone();
    version[4] = 3;
    assert_eq!(
        refused(&version),
        FormatError::Unsupported {
            what: "node-log blob",
            version: 3
        }
    );
    let mut negative = good.clone();
    negative[5] = 0x80;
    assert!(matches!(
        header_end(&negative),
        Err(FormatError::Corrupt { .. })
    ));
    let mut huge = good.clone();
    // The longest length the Java can write is read: a long header is the
    // caller's to fetch, by a ranged read sized from it.
    huge[5..9].copy_from_slice(&(MAX_HEADER_BYTES as i32).to_be_bytes());
    assert_eq!(
        header_end(&huge).unwrap(),
        Some(MAX_HEADER_BYTES as usize + PREFIX_BYTES)
    );
    // A fence's version is checked too, and a fence is complete in any bytes.
    let mut fence = encode_fence("n", 1).unwrap();
    assert_eq!(header_end(&fence[..4]).unwrap(), Some(4));
    fence[4] = 1;
    assert!(matches!(refused(&fence), FormatError::Unsupported { .. }));
    assert!(matches!(refused(&[]), FormatError::Truncated { .. }));
    assert!(matches!(
        refused(&FENCE_MAGIC.to_be_bytes()),
        FormatError::Truncated { .. }
    ));
}

#[test]
fn a_fence_holding_any_node_id_reads_back() {
    for id in ["n", "", "a\u{0}b", "é\u{1F600}"] {
        let fence = encode_fence(id, -7).unwrap();
        let NodeLogEntry::Fence(f) = decode(&fence).unwrap() else {
            panic!()
        };
        assert_eq!((f.fenced_by.as_ref(), f.term), (id, -7));
    }
    // NUL is two bytes, a character past U+FFFF six, as writeUTF writes them.
    assert_eq!(
        &encode_fence("\u{0}", 0).unwrap()[5..9],
        &[0, 2, 0xC0, 0x80]
    );
    assert_eq!(encode_fence("\u{1F600}", 0).unwrap()[5..7], [0, 6]);
    let long = "a".repeat(65_535);
    assert!(encode_fence(&long, 0).is_ok());
    assert!(matches!(
        encode_fence(&(long + "a"), 0),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn an_encoding_the_java_would_refuse_writes_nothing() {
    let shard = ShardKey::new("u", 0, Some("i"));
    let records = [WalRecord::index_unsequenced("1", "{}").encode().unwrap()];
    let batch = ShardBatch {
        shard: &shard,
        term: 1,
        records: &records,
    };
    let corrupt =
        |r: Result<_, FormatError>| assert!(matches!(r, Err(FormatError::Corrupt { .. })));
    corrupt(encode(&"n".repeat(65_536), 0, 0, 0, &[batch], &[]));
    corrupt(encode(
        "n",
        0,
        0,
        0,
        &[ShardBatch {
            shard: &ShardKey::new("u".repeat(65_536), 0, None::<&str>),
            ..batch
        }],
        &[],
    ));
    corrupt(encode("n", 1u64 << 63, 0, 0, &[batch], &[]));
    corrupt(encode("n", 0, 1u64 << 63, 0, &[batch], &[]));
    let big = ShardKey::new("u", 1u32 << 31, None::<&str>);
    corrupt(encode(
        "n",
        0,
        0,
        0,
        &[ShardBatch {
            shard: &big,
            ..batch
        }],
        &[],
    ));
    corrupt(encode(
        "n",
        0,
        0,
        0,
        &[],
        &[(big.clone(), vec![OrdinalRange { from: 0, to: 0 }])],
    ));
    let twice = [
        (shard.clone(), vec![OrdinalRange { from: 0, to: 0 }]),
        (shard.clone(), vec![OrdinalRange { from: 1, to: 1 }]),
    ];
    corrupt(encode("n", 0, 2, 0, &[], &twice));
}
