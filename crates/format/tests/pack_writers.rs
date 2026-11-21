//! The writers of a node pack's header and of a node's pack ledger, checked
//! against the pack the Java shell wrote (fixtures/shell-blobs) and against
//! this crate's own readers.

// ARITH: offsets here are fixture lengths and small constants; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use common::*;
use oss_format::{
    NodePackHeader, NodePackLedger, PackEpoch, PlannedBlock, encode_node_pack, encode_pack_ledger,
};

/// AC-1: the Java's pack header, re-encoded from its parsed blocks.
#[test]
fn the_javas_pack_header_is_re_encoded_byte_for_byte() {
    let pack = blob(&pack_key());
    let header = NodePackHeader::parse(&pack).unwrap();
    let planned: Vec<PlannedBlock<'_>> = header
        .blocks()
        .iter()
        .map(|b| PlannedBlock {
            index_uuid: &b.index_uuid,
            index_name: &b.index_name,
            shard: b.shard,
            name: &b.name,
            length: b.length,
        })
        .collect();
    let encoded = encode_node_pack(&header.node, header.epoch, header.ordinal, &planned).unwrap();
    let end = usize::try_from(header.body_start).unwrap();
    assert_eq!(encoded.bytes, pack[..end]);
    assert_eq!(encoded.offsets, vec![header.body_start]);
}

/// AC-2: blocks follow the header in order and read back as written.
#[test]
fn blocks_are_laid_out_after_the_header_in_order() {
    let planned = [
        PlannedBlock {
            index_uuid: "u1",
            index_name: "idx-é",
            shard: 0,
            name: "_0.seg",
            length: 10,
        },
        PlannedBlock {
            index_uuid: "u2",
            index_name: "",
            shard: 7,
            name: "_3.seg",
            length: 0,
        },
        PlannedBlock {
            index_uuid: "u1",
            index_name: "idx-é",
            shard: 1,
            name: "_1.seg",
            length: 5,
        },
    ];
    let encoded = encode_node_pack("n\0", 3, 4, &planned).unwrap();
    let parsed = NodePackHeader::parse(&encoded.bytes).unwrap();
    assert_eq!(
        (parsed.node.as_ref(), parsed.epoch, parsed.ordinal),
        ("n\0", 3, 4)
    );
    assert_eq!(parsed.body_start, encoded.bytes.len() as u64);
    let start = parsed.body_start;
    assert_eq!(encoded.offsets, vec![start, start + 10, start + 10]);
    for (block, (p, off)) in parsed
        .blocks()
        .iter()
        .zip(planned.iter().zip(&encoded.offsets))
    {
        assert_eq!(block.index_uuid, p.index_uuid);
        assert_eq!(block.index_name, p.index_name);
        assert_eq!((block.shard, block.name.as_ref()), (p.shard, p.name));
        assert_eq!((block.offset, block.length), (*off, p.length));
    }
}

/// AC-3: what no reader could follow is refused.
#[test]
fn a_pack_refuses_what_a_header_cannot_hold() {
    let ok = PlannedBlock {
        index_uuid: "u",
        index_name: "i",
        shard: 0,
        name: "a",
        length: 1,
    };
    assert!(encode_node_pack("n", 0, 0, &[]).is_err(), "no block");
    for bad in [
        PlannedBlock {
            index_uuid: "",
            ..ok
        },
        PlannedBlock { name: "", ..ok },
        PlannedBlock {
            length: u64::MAX,
            ..ok
        },
    ] {
        assert!(encode_node_pack("n", 0, 0, &[ok, bad]).is_err());
    }
    assert!(
        encode_node_pack("n", u64::MAX, 0, &[ok]).is_err(),
        "epoch past i64"
    );
    assert!(
        encode_node_pack("n", 0, u64::MAX, &[ok]).is_err(),
        "ordinal past i64"
    );
    let long = "x".repeat(70_000);
    assert!(
        encode_node_pack(&long, 0, 0, &[ok]).is_err(),
        "a string past writeUTF"
    );
    let many = vec![
        PlannedBlock {
            index_uuid: "uuid-uuid-uuid-uuid",
            index_name: "name-name-name",
            shard: 1,
            name: "_0123.seg",
            length: 1
        };
        70_000
    ];
    assert!(
        encode_node_pack("n", 0, 0, &many).is_err(),
        "a header past the limit"
    );
}

/// AC-4: the ledger reads back as written and is the Java's bytes.
#[test]
fn a_ledger_round_trips_and_is_the_javas_layout() {
    let epochs = vec![
        PackEpoch {
            epoch: 5,
            collected_below: 2,
            high: 9,
        },
        PackEpoch {
            epoch: 1_794_710_559_389,
            collected_below: 0,
            high: 0,
        },
    ];
    let bytes = encode_pack_ledger(&epochs).unwrap();
    let mut expect = vec![1u8, 0, 0, 0, 2];
    for (a, b, c) in [(5i64, 2i64, 9i64), (1_794_710_559_389, 0, 0)] {
        for v in [a, b, c] {
            expect.extend_from_slice(&v.to_be_bytes());
        }
    }
    assert_eq!(bytes, expect);
    assert_eq!(NodePackLedger::parse(&bytes).unwrap().epochs(), &epochs[..]);
    let empty = encode_pack_ledger(&[]).unwrap();
    assert_eq!(empty, [1, 0, 0, 0, 0]);
    assert!(NodePackLedger::parse(&empty).unwrap().epochs().is_empty());
}

/// AC-5: a ledger no reader could follow is refused.
#[test]
fn a_ledger_refuses_what_it_cannot_follow() {
    assert!(NodePackLedger::parse(&[]).is_err(), "empty");
    assert!(
        NodePackLedger::parse(&[2, 0, 0, 0, 0]).is_err(),
        "version 2"
    );
    assert!(NodePackLedger::parse(&[1, 0, 0, 0]).is_err(), "no count");
    assert!(
        NodePackLedger::parse(&[1, 0xff, 0xff, 0xff, 0xff]).is_err(),
        "negative count"
    );
    assert!(
        NodePackLedger::parse(&[1, 0, 0, 0, 1, 0, 0]).is_err(),
        "short epoch"
    );
    let mut with_negative = vec![1u8, 0, 0, 0, 1];
    with_negative.extend_from_slice(&(-1i64).to_be_bytes());
    with_negative.extend_from_slice(&[0; 16]);
    assert!(
        NodePackLedger::parse(&with_negative).is_err(),
        "negative epoch"
    );
    let mut trailing = encode_pack_ledger(&[]).unwrap();
    trailing.push(0);
    assert!(NodePackLedger::parse(&trailing).is_err(), "bytes left over");
    // a count the bytes cannot hold is refused before anything is allocated
    assert!(NodePackLedger::parse(&[1, 0x7f, 0xff, 0xff, 0xff]).is_err());
    assert!(
        encode_pack_ledger(&[PackEpoch {
            epoch: u64::MAX,
            collected_below: 0,
            high: 0
        }])
        .is_err()
    );
}

/// AC-3, at the edge: a header of exactly the limit is written, one byte more is refused.
#[test]
fn a_header_of_exactly_the_limit_is_written() {
    let block = PlannedBlock {
        index_uuid: "u",
        index_name: "i",
        shard: 0,
        name: "a",
        length: 1,
    };
    let blocks = vec![block; 144_000];
    // 22 bytes of node string and counters, 29 a block: 22 + 18_282 + 29 * 144_000 = 4 MiB.
    let at_limit = "n".repeat(18_282);
    let encoded = encode_node_pack(&at_limit, 0, 0, &blocks).unwrap();
    assert_eq!(encoded.bytes.len(), 9 + (4 << 20));
    let over = "n".repeat(18_283);
    assert!(encode_node_pack(&over, 0, 0, &blocks).is_err());
}
