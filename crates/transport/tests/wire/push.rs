//! The commit push and newest-commit messages, checked against bytes the
//! Java shell's own message classes wrote (`fixtures/transport/push.txt`).

use oss_transport::actions::push::{
    NEWEST_ACTION, NewestRequest, NewestResponse, PUSH_ACTION, PushRequest, PushResponse,
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

#[test]
fn every_recorded_push_message_is_the_javas_both_ways() {
    let mut seen = [0usize; 4];
    for row in rows("push.txt") {
        let encoded = bytes(row.last().unwrap());
        match row[0].as_str() {
            "push_request" => {
                seen[0] += 1;
                let blocks: usize = row[7].parse().unwrap();
                let expected = PushRequest {
                    parent: parent(&row[1], &row[2]),
                    index_name: unhex(&row[3]),
                    index_uuid: unhex(&row[4]),
                    shard: row[5].parse().unwrap(),
                    commit: bytes(&row[6]),
                    blocks: (0..blocks)
                        .map(|i| (format!("blk-{i}"), vec![i as u8, 0, 0xff, 7]))
                        .collect(),
                    mapping_version: row[8].parse().unwrap(),
                    settings_version: row[9].parse().unwrap(),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, PushRequest::read), expected);
            }
            "push_response" => {
                seen[1] += 1;
                let expected = PushResponse {
                    open: row[1].parse().unwrap(),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, PushResponse::read), expected);
            }
            "newest_request" => {
                seen[2] += 1;
                let expected = NewestRequest {
                    parent: parent(&row[1], &row[2]),
                    index_name: unhex(&row[3]),
                    index_uuid: unhex(&row[4]),
                    shard: row[5].parse().unwrap(),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, NewestRequest::read), expected);
            }
            "newest_response" => {
                seen[3] += 1;
                let expected = NewestResponse {
                    commit: (row[1] != "~").then(|| bytes(&row[1])),
                };
                assert_eq!(hex(&encode(|o| expected.write(o))), hex(&encoded));
                assert_eq!(read_whole(&encoded, NewestResponse::read), expected);
            }
            other => panic!("unknown row kind {other}"),
        }
    }
    assert_eq!(seen, [2, 2, 2, 3]);
}

#[test]
fn the_action_names_are_the_shells() {
    assert_eq!(PUSH_ACTION, "internal:serverless/commit/push");
    assert_eq!(NEWEST_ACTION, "internal:serverless/commit/newest");
}

#[test]
fn a_push_claiming_more_blocks_than_it_holds_is_refused_before_it_allocates() {
    let mut encoded = encode(|o| {
        PushRequest {
            commit: b"{}".to_vec(),
            ..PushRequest::default()
        }
        .write(o)
    });
    // The block count is the byte before the two zlong versions (1 byte each,
    // -1 is 0x01 as a zlong) when zero: 1_000_000 blocks in five bytes.
    let at = encoded.len() - 3;
    assert_eq!(encoded[at], 0);
    encoded.splice(at..=at, [0xc0, 0x84, 0x3d]);
    assert!(PushRequest::read(&mut Input::new(&encoded)).is_err());
}

#[test]
fn a_commit_of_nothing_and_no_commit_are_told_apart() {
    let none = encode(|o| NewestResponse { commit: None }.write(o));
    let empty = encode(|o| {
        NewestResponse {
            commit: Some(Vec::new()),
        }
        .write(o)
    });
    assert_eq!(
        (none.as_slice(), empty.as_slice()),
        (&[0u8][..], &[1u8][..])
    );
    assert_eq!(
        NewestResponse::read(&mut Input::new(&empty))
            .unwrap()
            .commit,
        Some(Vec::new())
    );
    assert_eq!(
        NewestResponse::read(&mut Input::new(&none)).unwrap().commit,
        None
    );
}
