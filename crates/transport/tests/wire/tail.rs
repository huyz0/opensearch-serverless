//! The tail fetch and tail push messages, checked against bytes the Java
//! shell's own message classes wrote (`fixtures/transport/tail.txt`).

use oss_transport::actions::tail::{
    CHECK, FETCH_ACTION, FetchRequest, FetchResponse, NEW, NONE, PUSH_ACTION, PushRequest,
    PushResponse, Record, ShardRef,
};
use oss_transport::error::WireError;
use oss_transport::stream::{Input, Output, TaskId};

use crate::{bytes, hex, rows, unhex};

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

fn parent(node: &str, id: &str) -> TaskId {
    let node = unhex(node);
    TaskId {
        task: (!node.is_empty()).then(|| (node, id.parse().unwrap())),
    }
}

/// `text`: UTF-8 hex, `~` for none.
fn opt(text: &str) -> Option<String> {
    (text != "~").then(|| unhex(text))
}

fn records(column: &str) -> Vec<Record> {
    if column == "-" {
        return Vec::new();
    }
    column
        .split(';')
        .map(|item| {
            let f: Vec<&str> = item.split(',').collect();
            let deletion: bool = f[1].parse().unwrap();
            Record {
                id: unhex(f[0]),
                deletion,
                source: unhex(f[2]),
                seq_no: f[3].parse().unwrap(),
                primary_term: f[4].parse().unwrap(),
                version: f[5].parse().unwrap(),
                routing: opt(f[6]),
            }
        })
        .collect()
}

fn shard(f: &[&str]) -> ShardRef {
    ShardRef {
        index_name: unhex(f[0]),
        index_uuid: unhex(f[1]),
        shard: f[2].parse().unwrap(),
    }
}

#[test]
fn every_recorded_tail_message_is_the_javas_both_ways() {
    let mut seen = [0usize; 4];
    for row in rows("tail.txt") {
        let encoded = bytes(row.last().unwrap());
        match row[0].as_str() {
            "fetch_request" => {
                seen[0] += 1;
                let expected = FetchRequest {
                    parent: parent(&row[1], &row[2]),
                    shard: ShardRef {
                        index_name: unhex(&row[3]),
                        index_uuid: unhex(&row[4]),
                        shard: row[5].parse().unwrap(),
                    },
                    after_seq_no: row[6].parse().unwrap(),
                    subscriber: unhex(&row[7]),
                    subscription: row[8].parse().unwrap(),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, FetchRequest::read), expected);
            }
            "fetch_response" => {
                seen[1] += 1;
                let expected = FetchResponse {
                    records: records(&row[1]),
                    subscription: row[2].parse().unwrap(),
                    complete: row[3].parse().unwrap(),
                    claimed_through: row[4].parse().unwrap(),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, FetchResponse::read), expected);
            }
            "push_request" => {
                seen[2] += 1;
                let shards = if row[3] == "-" {
                    Vec::new()
                } else {
                    row[3]
                        .split('/')
                        .map(|s| {
                            let f: Vec<&str> = s.splitn(4, '|').collect();
                            (shard(&f), records(f[3]))
                        })
                        .collect()
                };
                let expected = PushRequest {
                    parent: parent(&row[1], &row[2]),
                    shards,
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, PushRequest::read), expected);
            }
            "push_response" => {
                seen[3] += 1;
                let unwanted = if row[1] == "-" {
                    Vec::new()
                } else {
                    row[1]
                        .split('/')
                        .map(|s| shard(&s.split('|').collect::<Vec<_>>()))
                        .collect()
                };
                let expected = PushResponse { unwanted };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, PushResponse::read), expected);
            }
            other => panic!("unknown row kind {other}"),
        }
    }
    assert_eq!(seen, [3, 4, 3, 2]);
}

#[test]
fn the_action_names_and_sentinels_are_the_shells() {
    assert_eq!(FETCH_ACTION, "internal:serverless/tail/fetch");
    assert_eq!(PUSH_ACTION, "internal:serverless/tail/push");
    assert_eq!((NEW, CHECK, NONE), (-1, -2, -1));
}

#[test]
fn a_deletion_travels_without_its_source() {
    let record = Record {
        id: "d".into(),
        deletion: true,
        source: "ignored".into(),
        seq_no: 3,
        primary_term: 1,
        version: 2,
        routing: None,
    };
    let encoded = encode(|o| {
        FetchResponse {
            records: vec![record],
            ..FetchResponse::default()
        }
        .write(o)
    });
    let back = FetchResponse::read(&mut Input::new(&encoded)).unwrap();
    assert_eq!(back.records[0].source, "");
    assert!(back.records[0].deletion);
}

#[test]
fn a_message_claiming_more_than_its_bytes_hold_is_refused_before_it_allocates() {
    // 1,000,000 as a vint: c0 84 3d.
    let many = [0xc0, 0x84, 0x3d];
    assert!(FetchResponse::read(&mut Input::new(&many)).is_err());
    assert!(PushResponse::read(&mut Input::new(&many)).is_err());
    let mut push = encode(|o| PushRequest::default().write(o));
    let at = push.len() - 1;
    assert_eq!(push[at], 0);
    push.splice(at..=at, many);
    assert!(PushRequest::read(&mut Input::new(&push)).is_err());
    // One shard that claims 1,000,000 records.
    let mut shard = encode(|o| {
        PushRequest {
            shards: vec![(ShardRef::default(), Vec::new())],
            ..PushRequest::default()
        }
        .write(o)
    });
    let at = shard.len() - 1;
    assert_eq!(shard[at], 0);
    shard.splice(at..=at, many);
    assert!(PushRequest::read(&mut Input::new(&shard)).is_err());
}
