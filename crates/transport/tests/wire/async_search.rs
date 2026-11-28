//! The forwarded asynchronous search call: its own layout, and round trips.

use oss_transport::actions::async_search::{
    ASYNC_SEARCH_ACTION, AsyncSearchRequest, AsyncSearchResponse, Op,
};
use oss_transport::error::WireError;
use oss_transport::stream::{Input, Output};

fn encode(write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>) -> Vec<u8> {
    let mut buf = Vec::new();
    write(&mut Output::new(&mut buf, 8 << 20)).unwrap();
    buf
}

fn request(op: Op, keep_alive_ms: Option<u64>) -> AsyncSearchRequest {
    AsyncSearchRequest {
        op,
        number: 1_700_000_000_123,
        tenant: "t".into(),
        user: "alice".into(),
        wait_ms: 1_000,
        keep_alive_ms,
    }
}

#[test]
fn the_action_is_in_the_serverless_namespace() {
    assert_eq!(ASYNC_SEARCH_ACTION, "internal:serverless/async_search/call");
}

#[test]
fn a_request_round_trips_with_its_caller_and_refuses_every_cut() {
    for r in [
        request(Op::Get, None),
        request(Op::Get, Some(60_000)),
        request(Op::Delete, None),
    ] {
        let bytes = encode(|o| r.write(o));
        assert_eq!(bytes[0], u8::from(r.op == Op::Delete));
        for cut in 0..bytes.len() {
            assert!(
                AsyncSearchRequest::read(&mut Input::new(&bytes[..cut])).is_err(),
                "cut {cut}"
            );
        }
        assert_eq!(
            AsyncSearchRequest::read(&mut Input::new(&bytes)).unwrap(),
            r
        );
    }
}

#[test]
fn an_unknown_op_and_a_number_past_i64_are_refused() {
    let mut bytes = encode(|o| request(Op::Get, None).write(o));
    bytes[0] = 2;
    assert!(AsyncSearchRequest::read(&mut Input::new(&bytes)).is_err());
    let mut too_big = request(Op::Get, None);
    too_big.number = u64::MAX;
    assert!(
        too_big
            .write(&mut Output::new(&mut Vec::new(), 64))
            .is_err()
    );
}

#[test]
fn an_answer_is_a_status_and_a_body_and_refuses_every_cut() {
    let answer = AsyncSearchResponse {
        status: 404,
        body: r#"{"error":{}}"#.into(),
    };
    let bytes = encode(|o| answer.write(o));
    for cut in 0..bytes.len() {
        assert!(
            AsyncSearchResponse::read(&mut Input::new(&bytes[..cut])).is_err(),
            "cut {cut}"
        );
    }
    assert_eq!(
        AsyncSearchResponse::read(&mut Input::new(&bytes)).unwrap(),
        answer
    );
    for status in [0, 99, 600, -1] {
        let bad = encode(|o| {
            o.vint(status)?;
            o.string("{}")
        });
        assert!(
            AsyncSearchResponse::read(&mut Input::new(&bad)).is_err(),
            "{status}"
        );
    }
    let huge = AsyncSearchResponse {
        status: 200,
        body: "x".repeat((2 << 20) + 1),
    };
    assert!(
        huge.write(&mut Output::new(&mut Vec::new(), 8 << 20))
            .is_err()
    );
}
