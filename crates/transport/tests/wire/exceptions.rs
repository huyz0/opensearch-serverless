//! Exceptions: what a failed request answers.

use std::net::{IpAddr, Ipv4Addr};

use oss_transport::WireError;
use oss_transport::exception::{ExceptionKind, MAX_DEPTH, RemoteException, ids};
use oss_transport::frame::{self, FrameMeta, Status, VariableHeader};
use oss_transport::node::TransportAddress;
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;

use crate::{bytes, hex, rows};

fn shell_address() -> TransportAddress {
    TransportAddress {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
        host: "10.0.0.7".to_owned(),
        port: 9300,
    }
}

const ACTION: &str = "internal:serverless/document/write";

/// The failure a `bare` or `frame` row describes, wrapped as the Java wraps it.
fn failure(description: &str) -> RemoteException {
    let (kind, rest) = description.split_once(':').unwrap();
    let cause = match kind {
        "security" => {
            let (message, status) = rest.rsplit_once(':').unwrap();
            RemoteException::security(message, status)
        }
        "status" => {
            let (message, status) = rest.rsplit_once(':').unwrap();
            RemoteException::status(message, status)
        }
        "rejected" => RemoteException::rejected(rest),
        "not_found" => RemoteException::action_not_found(rest),
        other => panic!("unknown failure {other}"),
    };
    RemoteException::remote("w1", shell_address(), ACTION, cause)
}

fn read(encoded: &[u8]) -> Option<RemoteException> {
    let mut input = Input::new(encoded);
    let out = RemoteException::read(&mut input).unwrap();
    if out.as_ref().is_none_or(|e| e.complete) {
        input.finish("an exception").unwrap();
    }
    out
}

#[test]
fn the_failures_a_node_answers_are_written_as_the_java_writes_them() {
    let rows = rows("exceptions.txt");
    let bare: Vec<_> = rows.iter().filter(|r| r[0] == "bare").collect();
    assert_eq!(bare.len(), 4);
    for row in bare {
        let expected = failure(&row[1]);
        let mut buf = Vec::new();
        expected.write(&mut Output::new(&mut buf, 4096)).unwrap();
        assert_eq!(hex(&buf), row[2], "{}", row[1]);
        let encoded = bytes(&row[2]);
        assert_eq!(read(&encoded).unwrap(), expected);
        for cut in 0..encoded.len() {
            assert!(RemoteException::read(&mut Input::new(&encoded[..cut])).is_err());
        }
    }
}

#[test]
fn every_recorded_chain_is_read() {
    let rows = rows("exceptions.txt");
    let mut seen = 0;
    for row in rows.iter().filter(|r| r[0] == "read") {
        seen += 1;
        let encoded = bytes(&row[2]);
        let description = row[1].as_str();
        if description == "none" {
            assert_eq!(read(&encoded), None);
            continue;
        }
        let outer = read(&encoded).unwrap();
        assert_eq!(
            outer.kind,
            Some(ExceptionKind::OpenSearch(ids::REMOTE_TRANSPORT))
        );
        let root = outer.root();
        let (kind, rest) = description.split_once(':').unwrap();
        match kind {
            "jdk" => {
                let key: i32 = rest.split(':').next().unwrap().parse().unwrap();
                assert_eq!(root.kind, Some(ExceptionKind::Jdk(key)), "{description}");
                assert!(outer.complete, "{description}");
                assert_eq!(outer.action.as_deref(), Some(ACTION));
                assert_eq!(outer.address.as_deref(), Some(&shell_address()));
            }
            "status" => {
                assert_eq!(root.kind, Some(ExceptionKind::OpenSearch(ids::STATUS)));
                assert_eq!(root.message.as_deref(), Some("with metadata"));
                assert_eq!(root.status.as_deref(), Some("CONFLICT"));
                assert_eq!(outer.http_status(), 409);
                // Its cause chain: illegal argument, an I/O failure, an illegal state.
                let cause = root.cause.as_deref().unwrap();
                assert_eq!(cause.kind, Some(ExceptionKind::Jdk(6)));
                let io = cause.cause.as_deref().unwrap();
                assert_eq!(io.kind, Some(ExceptionKind::Jdk(17)));
                assert_eq!(
                    io.cause.as_deref().unwrap().kind,
                    Some(ExceptionKind::Jdk(14))
                );
                assert!(outer.complete);
            }
            "unknown" => {
                assert_eq!(root.kind, Some(ExceptionKind::OpenSearch(12)));
                assert!(!outer.complete && !root.complete);
                assert_eq!(outer.http_status(), 500);
            }
            "wrapped" => {
                // NotSerializableExceptionWrapper: the throwable's name and a status.
                assert_eq!(
                    root.kind,
                    Some(ExceptionKind::OpenSearch(ids::NOT_SERIALIZABLE))
                );
                assert_eq!(root.status.as_deref(), Some("INTERNAL_SERVER_ERROR"));
                assert!(
                    root.message
                        .as_deref()
                        .unwrap()
                        .contains("changed underneath"),
                    "{:?}",
                    root.message
                );
                assert!(outer.complete);
                assert_eq!(outer.http_status(), 500);
            }
            other => panic!("unknown row {other}"),
        }
    }
    assert_eq!(seen, 22);
    let jdk6 = rows.iter().find(|r| r[1] == "jdk:6:bad argument").unwrap();
    let outer = read(&bytes(&jdk6[2])).unwrap();
    assert_eq!(outer.root().message.as_deref(), Some("bad argument"));
    assert_eq!(outer.http_status(), 400);
}

#[test]
fn an_error_response_frame_is_the_javas() {
    let rows = rows("exceptions.txt");
    let row = rows.iter().find(|r| r[0] == "frame").unwrap();
    let failure = failure(&row[1]);
    let meta = FrameMeta {
        request_id: 42,
        status: Status::response(false, false, true),
        version: WireVersion::SHELL,
    };
    let mut buf = Vec::new();
    frame::encode(&mut buf, 1 << 16, meta, &VariableHeader::default(), |o| {
        failure.write(o)
    })
    .unwrap();
    assert_eq!(hex(&buf), row[2]);
    let decoded = frame::decode(&buf, 1 << 16).unwrap();
    assert!(decoded.header.status.is_error());
    let read = RemoteException::read(&mut Input::new(&decoded.message))
        .unwrap()
        .unwrap();
    assert_eq!(read.http_status(), 403);
    assert_eq!(
        read.message.as_deref(),
        Some("[w1][10.0.0.7:9300][internal:serverless/document/write]")
    );
}

#[test]
fn statuses_map_as_opensearch_maps_them() {
    let cases = [
        (RemoteException::rejected("busy"), 429),
        (RemoteException::action_not_found("a"), 500),
        (RemoteException::status("x", "SERVICE_UNAVAILABLE"), 503),
        (RemoteException::status("x", "NO_SUCH_STATUS"), 500),
        (RemoteException::security("x", "UNAUTHORIZED"), 401),
    ];
    for (e, code) in cases {
        assert_eq!(e.http_status(), code, "{e:?}");
    }
    for (name, code) in [
        ("BAD_REQUEST", 400),
        ("NOT_FOUND", 404),
        ("GONE", 410),
        ("REQUEST_ENTITY_TOO_LARGE", 413),
        ("TOO_MANY_REQUESTS", 429),
        ("INTERNAL_SERVER_ERROR", 500),
        ("GATEWAY_TIMEOUT", 504),
    ] {
        assert_eq!(RemoteException::status("x", name).http_status(), code);
    }
    assert_eq!(
        RemoteException::action_not_found("a").message.as_deref(),
        Some("No handler for action [a]")
    );
    // A remote exception over IPv6 names its address in brackets.
    let v6 = TransportAddress {
        ip: "::1".parse().unwrap(),
        host: "::1".to_owned(),
        port: 1,
    };
    let remote = RemoteException::remote("n", v6, "a", RemoteException::rejected("r"));
    assert_eq!(remote.message.as_deref(), Some("[n][[::1]:1][a]"));
    assert_eq!(remote.http_status(), 429);
}

#[test]
fn what_cannot_be_written_or_read_is_refused() {
    let mut buf = Vec::new();
    let jdk = RemoteException {
        kind: Some(ExceptionKind::Jdk(6)),
        ..RemoteException::default()
    };
    assert!(matches!(
        jdk.write(&mut Output::new(&mut buf, 64)),
        Err(WireError::Malformed { .. })
    ));
    let unknown = RemoteException {
        kind: Some(ExceptionKind::OpenSearch(12)),
        ..RemoteException::default()
    };
    buf.clear();
    assert!(matches!(
        unknown.write(&mut Output::new(&mut buf, 64)),
        Err(WireError::Malformed { .. })
    ));
    let transport = RemoteException {
        kind: Some(ExceptionKind::OpenSearch(ids::TRANSPORT)),
        message: Some("t".to_owned()),
        complete: true,
        ..RemoteException::default()
    };
    buf.clear();
    transport.write(&mut Output::new(&mut buf, 64)).unwrap();
    assert_eq!(read(&buf).unwrap(), transport);
    // An unknown key, and a chain past the depth limit.
    assert!(matches!(
        RemoteException::read(&mut Input::new(&[1, 19])),
        Err(WireError::Malformed { .. })
    ));
    let mut deep = Vec::new();
    for _ in 0..=MAX_DEPTH {
        // An illegal argument with no message, whose cause follows.
        deep.extend_from_slice(&[1, 6, 0]);
    }
    deep.push(0);
    assert!(matches!(
        RemoteException::read(&mut Input::new(&deep)),
        Err(WireError::Malformed { .. })
    ));
}
