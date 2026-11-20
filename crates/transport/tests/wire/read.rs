//! The forwarded get: layouts written out from the Java shell's
//! `ForwardedGetRequest` and `ForwardedGetResponse` (not recorded from a
//! build of them), and round trips.

use oss_transport::actions::read::{GET_ACTION, GetRequest, GetResponse};
use oss_transport::error::WireError;
use oss_transport::stream::{Input, Output, TaskId};

fn encode(write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>) -> Vec<u8> {
    let mut buf = Vec::new();
    write(&mut Output::new(&mut buf, 1 << 20)).unwrap();
    buf
}

/// Reads `encoded` whole, and refuses every shorter cut.
fn read_whole<T>(encoded: &[u8], read: impl Fn(&mut Input<'_>) -> Result<T, WireError>) -> T {
    for cut in 0..encoded.len() {
        assert!(read(&mut Input::new(&encoded[..cut])).is_err(), "cut {cut}");
    }
    let mut input = Input::new(encoded);
    let value = read(&mut input).unwrap();
    assert_eq!(input.remaining(), 0);
    value
}

#[test]
fn the_get_action_name_is_the_javas() {
    assert_eq!(GET_ACTION, "internal:serverless/document/get");
}

#[test]
fn a_get_request_is_the_task_id_index_uuid_shard_and_id() {
    let request = GetRequest {
        parent: TaskId::EMPTY,
        index: "logs".into(),
        index_uuid: None,
        shard: 2,
        id: "a".into(),
    };
    let bytes = encode(|o| request.write(o));
    // The empty task id; "logs"; no uuid; vint 2; "a".
    assert_eq!(
        bytes,
        [0x00, 0x04, b'l', b'o', b'g', b's', 0x00, 0x02, 0x01, b'a']
    );
    assert_eq!(read_whole(&bytes, GetRequest::read), request);
}

#[test]
fn a_get_request_names_its_incarnation_and_parent() {
    let request = GetRequest {
        parent: TaskId {
            task: Some(("n1".into(), 7)),
        },
        index: "logs".into(),
        index_uuid: Some("u1".into()),
        shard: 0,
        id: "b".into(),
    };
    let bytes = encode(|o| request.write(o));
    assert_eq!(read_whole(&bytes, GetRequest::read), request);
}

#[test]
fn a_get_response_is_owner_id_found_source_sequence_identity_and_routing() {
    let response = GetResponse {
        owner_node_id: "n1".into(),
        id: "a".into(),
        found: true,
        source: Some("{}".into()),
        seq_no: 5,
        primary_term: 3,
        version: 1,
        routing: None,
    };
    let bytes = encode(|o| response.write(o));
    // "n1"; "a"; true; present "{}"; zlong 5 = 0x0a; vlong 3; zlong 1 = 0x02; no routing.
    assert_eq!(
        bytes,
        [
            0x02, b'n', b'1', 0x01, b'a', 0x01, 0x01, 0x02, b'{', b'}', 0x0a, 0x03, 0x02, 0x00
        ]
    );
    assert_eq!(read_whole(&bytes, GetResponse::read), response);
}

#[test]
fn a_get_response_for_a_missing_document_carries_the_unassigned_identity() {
    let response = GetResponse {
        owner_node_id: "n1".into(),
        id: "zz".into(),
        found: false,
        source: None,
        seq_no: -2,
        primary_term: 0,
        version: -1,
        routing: Some("r".into()),
    };
    let bytes = encode(|o| response.write(o));
    assert_eq!(read_whole(&bytes, GetResponse::read), response);
}
