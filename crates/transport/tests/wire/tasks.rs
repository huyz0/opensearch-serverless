//! The forwarded task call: its own layout, and round trips.

use oss_transport::actions::tasks::{TASKS_ACTION, TaskRequest, TaskResponse};
use oss_transport::error::WireError;
use oss_transport::stream::{Input, Output};

fn encode(write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>) -> Vec<u8> {
    let mut buf = Vec::new();
    write(&mut Output::new(&mut buf, 1 << 20)).unwrap();
    buf
}

#[test]
fn the_action_is_in_the_serverless_namespace() {
    assert_eq!(TASKS_ACTION, "internal:serverless/tasks/call");
}

#[test]
fn a_request_is_a_flag_and_a_number_and_refuses_every_cut() {
    for request in [
        TaskRequest {
            cancel: false,
            number: 1_700_000_000_123,
        },
        TaskRequest {
            cancel: true,
            number: 0,
        },
    ] {
        let bytes = encode(|o| request.write(o));
        assert_eq!(bytes[0], u8::from(request.cancel));
        for cut in 0..bytes.len() {
            assert!(
                TaskRequest::read(&mut Input::new(&bytes[..cut])).is_err(),
                "cut {cut}"
            );
        }
        assert_eq!(TaskRequest::read(&mut Input::new(&bytes)).unwrap(), request);
    }
    let too_big = TaskRequest {
        cancel: false,
        number: u64::MAX,
    };
    assert!(
        too_big
            .write(&mut Output::new(&mut Vec::new(), 64))
            .is_err()
    );
    // A variable-length long with its top bit set: a negative number.
    let mut negative = vec![0u8];
    negative.extend([0xff; 9]);
    negative.push(0x01);
    assert!(TaskRequest::read(&mut Input::new(&negative)).is_err());
}

#[test]
fn an_answer_is_a_status_and_a_body_and_refuses_every_cut() {
    let answer = TaskResponse {
        status: 404,
        body: r#"{"error":{}}"#.into(),
    };
    let bytes = encode(|o| answer.write(o));
    for cut in 0..bytes.len() {
        assert!(
            TaskResponse::read(&mut Input::new(&bytes[..cut])).is_err(),
            "cut {cut}"
        );
    }
    assert_eq!(TaskResponse::read(&mut Input::new(&bytes)).unwrap(), answer);
    for status in [0, 99, 600, -1] {
        let bad = encode(|o| {
            o.vint(status)?;
            o.string("{}")
        });
        assert!(
            TaskResponse::read(&mut Input::new(&bad)).is_err(),
            "{status}"
        );
    }
    let huge = TaskResponse {
        status: 200,
        body: "x".repeat((1 << 17) + 1),
    };
    assert!(
        huge.write(&mut Output::new(&mut Vec::new(), 1 << 20))
            .is_err()
    );
}
