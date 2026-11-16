//! The forwarded writes, checked against bytes the Java shell's own message
//! classes wrote (`fixtures/transport/forwarding.txt`).

use oss_transport::actions::write::{
    BulkOperation, BulkOutcome, BulkRequest, BulkResponse, IndexRequest, IndexResponse,
    UNASSIGNED_SEQ_NO,
};
use oss_transport::error::WireError;
use oss_transport::stream::{Input, Output, TaskId};

use crate::{bytes, hex, rows, unhex};

/// A recorded string: `~` for null, `-` for empty, else UTF-8 hex.
fn text(field: &str) -> Option<String> {
    (field != "~").then(|| unhex(field))
}

fn flag(field: &str) -> bool {
    field.parse().unwrap()
}

fn encode(write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>) -> Vec<u8> {
    let mut buf = Vec::new();
    write(&mut Output::new(&mut buf, 1 << 20)).unwrap();
    buf
}

/// Reads `encoded` whole with `read`, and refuses every shorter cut.
fn read_whole<T>(encoded: &[u8], read: impl Fn(&mut Input<'_>) -> Result<T, WireError>) -> T {
    for cut in 0..encoded.len() {
        let mut input = Input::new(&encoded[..cut]);
        assert!(read(&mut input).is_err(), "a cut at {cut} reads");
    }
    let mut input = Input::new(encoded);
    let value = read(&mut input).unwrap();
    assert_eq!(input.remaining(), 0);
    value
}

#[test]
fn every_recorded_forwarded_write_is_the_javas_both_ways() {
    let mut seen = [0usize; 4];
    for row in rows("forwarding.txt") {
        let encoded = bytes(row.last().unwrap());
        match row[0].as_str() {
            "index_request" => {
                seen[0] += 1;
                let node = unhex(&row[1]);
                let expected = IndexRequest {
                    parent: TaskId {
                        task: (!node.is_empty()).then(|| (node, row[2].parse().unwrap())),
                    },
                    index: unhex(&row[3]),
                    shard: row[4].parse().unwrap(),
                    id: unhex(&row[5]),
                    source: unhex(&row[6]),
                    refresh: flag(&row[7]),
                    deletion: flag(&row[8]),
                    if_seq_no: row[9].parse().unwrap(),
                    if_primary_term: row[10].parse().unwrap(),
                    require_absent: flag(&row[11]),
                    index_uuid: text(&row[12]),
                    steered: flag(&row[13]),
                    routing: text(&row[14]),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, IndexRequest::read), expected);
            }
            "index_response" => {
                seen[1] += 1;
                let expected = IndexResponse {
                    owner_node_id: unhex(&row[1]),
                    seq_no: row[2].parse().unwrap(),
                    primary_term: row[3].parse().unwrap(),
                    version: row[4].parse().unwrap(),
                    created: flag(&row[5]),
                    found: flag(&row[6]),
                    refreshed: flag(&row[7]),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, IndexResponse::read), expected);
            }
            "bulk_request" => {
                seen[2] += 1;
                let operations = if row[3] == "-" {
                    Vec::new()
                } else {
                    row[3]
                        .split(',')
                        .map(|op| {
                            let f: Vec<&str> = op.split('|').collect();
                            BulkOperation {
                                id: unhex(f[0]),
                                source: unhex(f[1]),
                                deletion: flag(f[2]),
                                if_seq_no: f[3].parse().unwrap(),
                                if_primary_term: f[4].parse().unwrap(),
                                require_absent: flag(f[5]),
                                routing: text(f[6]),
                            }
                        })
                        .collect()
                };
                let expected = BulkRequest {
                    parent: TaskId::EMPTY,
                    index: unhex(&row[1]),
                    shard: row[2].parse().unwrap(),
                    operations,
                    refresh: flag(&row[4]),
                    index_uuid: text(&row[5]),
                    steered: flag(&row[6]),
                    tail_push: flag(&row[7]),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, BulkRequest::read), expected);
            }
            "bulk_response" => {
                seen[3] += 1;
                let outcomes = if row[2] == "-" {
                    Vec::new()
                } else {
                    row[2]
                        .split(',')
                        .map(|o| {
                            let f: Vec<&str> = o.split('|').collect();
                            BulkOutcome {
                                id: unhex(f[0]),
                                failure: text(f[1]),
                                conflict: flag(f[2]),
                                retryable: flag(f[3]),
                                deletion: flag(f[4]),
                                found: flag(f[5]),
                                seq_no: f[6].parse().unwrap(),
                                primary_term: f[7].parse().unwrap(),
                                version: f[8].parse().unwrap(),
                                created: flag(f[9]),
                            }
                        })
                        .collect()
                };
                let expected = BulkResponse {
                    owner_node_id: unhex(&row[1]),
                    outcomes,
                    refreshed: flag(&row[3]),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, BulkResponse::read), expected);
            }
            other => panic!("unknown row {other}"),
        }
    }
    assert_eq!(seen, [4, 3, 3, 2]);
}

#[test]
fn a_count_the_bytes_cannot_hold_is_refused_before_anything_is_sized() {
    // A bulk claiming a million operations in a few bytes.
    let mut claim = encode(|o| {
        TaskId::EMPTY.write(o)?;
        o.string("logs")?;
        o.vint(0)?;
        o.vint(1_000_000)
    });
    claim.extend_from_slice(&[0; 16]);
    assert!(matches!(
        BulkRequest::read(&mut Input::new(&claim)),
        Err(WireError::Truncated { .. })
    ));
    let claim = encode(|o| {
        o.string("owner")?;
        o.vint(1_000_000)
    });
    assert!(matches!(
        BulkResponse::read(&mut Input::new(&claim)),
        Err(WireError::Truncated { .. })
    ));
    // The smallest operation and outcome each fit their bound exactly.
    let one = BulkRequest {
        operations: vec![BulkOperation::default()],
        ..BulkRequest::default()
    };
    let encoded = encode(|o| one.write(o));
    assert_eq!(BulkRequest::read(&mut Input::new(&encoded)).unwrap(), one);
    let one = BulkResponse {
        outcomes: vec![BulkOutcome::default()],
        ..BulkResponse::default()
    };
    let encoded = encode(|o| one.write(o));
    assert_eq!(BulkResponse::read(&mut Input::new(&encoded)).unwrap(), one);
}

/// Reads a bulk or its answer claiming `count` items with `left` zero bytes
/// after the count (each zero item is the smallest valid one), and answers
/// how many bytes were still unread when the read failed: all of them when
/// the count was refused, none when the items were read and the trailer
/// was missing.
fn unread_after_claim(response: bool, count: i32, left: usize) -> usize {
    let mut claim = encode(|o| {
        if response {
            o.string("owner")?;
        } else {
            TaskId::EMPTY.write(o)?;
            o.string("logs")?;
            o.vint(0)?;
        }
        o.vint(count)
    });
    claim.extend(std::iter::repeat_n(0u8, left));
    let mut input = Input::new(&claim);
    let failed = if response {
        BulkResponse::read(&mut input).is_err()
    } else {
        BulkRequest::read(&mut input).is_err()
    };
    assert!(failed, "no trailer, so the read fails");
    input.remaining()
}

#[test]
fn a_count_is_held_to_seven_bytes_an_operation_and_ten_an_outcome() {
    assert_eq!(
        unread_after_claim(false, 3, 21),
        0,
        "21 bytes may hold 3 operations"
    );
    assert_eq!(unread_after_claim(false, 3, 20), 20, "20 bytes cannot");
    assert_eq!(
        unread_after_claim(true, 3, 30),
        0,
        "30 bytes may hold 3 outcomes"
    );
    assert_eq!(unread_after_claim(true, 3, 29), 29, "29 bytes cannot");
}

#[test]
fn a_deletion_travels_without_its_source_as_the_java_rebuilds_it() {
    let request = BulkRequest {
        operations: vec![BulkOperation {
            id: "a".into(),
            source: "{\"stale\":1}".into(),
            deletion: true,
            ..BulkOperation::default()
        }],
        ..BulkRequest::default()
    };
    let encoded = encode(|o| request.write(o));
    let mut expected = request.clone();
    expected.operations[0].source.clear();
    assert_eq!(
        encode(|o| expected.write(o)),
        encoded,
        "the source is not written"
    );
    assert_eq!(read_whole(&encoded, BulkRequest::read), expected);
    // A deletion read with a source, as an older sender might write it,
    // keeps none, as `WalRecord.deletion(id)` keeps none.
    let with_source = encode(|o| {
        TaskId::EMPTY.write(o)?;
        o.string("")?;
        o.vint(0)?;
        o.vint(1)?;
        o.string("a")?;
        o.string("{}")?;
        o.bool(true)?;
        o.zlong(-2)?;
        o.vlong(0)?;
        o.bool(false)?;
        o.optional_string(None)?;
        o.bool(false)?;
        o.optional_string(None)?;
        o.bool(false)?;
        o.bool(false)
    });
    let read = read_whole(&with_source, BulkRequest::read);
    assert_eq!(read.operations[0].source, "");
    assert!(read.operations[0].deletion);
}

#[test]
fn no_seq_no_condition_is_the_javas_unassigned_seq_no() {
    // `SequenceNumbers.UNASSIGNED_SEQ_NO`, as a zlong: 3.
    assert_eq!(UNASSIGNED_SEQ_NO, -2);
    assert_eq!(encode(|o| o.zlong(UNASSIGNED_SEQ_NO)), [3]);
}
