//! AC-4 and AC-5: frames, the handshakes and compression.

use oss_transport::WireError;
use oss_transport::frame::{
    self, FIXED_HEADER_BYTES, FixedHeader, FrameMeta, PING, Prefix, Status, VariableHeader,
};
use oss_transport::handshake::{
    TCP_HANDSHAKE_ACTION, TRANSPORT_HANDSHAKE_ACTION, TcpHandshakeRequest, TcpHandshakeResponse,
    TransportHandshakeRequest, TransportHandshakeResponse,
};
use oss_transport::stream::{Input, Output, TaskId};
use oss_transport::version::WireVersion;

use crate::{bytes, hex, node, rows, unhex};

const MAX: usize = 1 << 20;

/// The variable header a row's headers, features and action columns name.
fn variable(row: &[String], is_request: bool) -> VariableHeader {
    let mut out = VariableHeader::default();
    if row[3] != "-" {
        for part in row[3].split(';') {
            let (kind, kv) = part.split_at(2);
            let (k, v) = kv.split_once('=').unwrap();
            match kind {
                "q:" => out.request_headers.push((k.to_owned(), v.to_owned())),
                "r:" => out
                    .response_headers
                    .push((k.to_owned(), v.split('|').map(str::to_owned).collect())),
                other => panic!("unknown header kind {other}"),
            }
        }
    }
    if is_request {
        if row[4] != "-" {
            out.features = row[4].split(',').map(str::to_owned).collect();
        }
        out.action = row[5].clone();
    }
    out
}

/// A generic message row's description: `parent=<node:id or ->;string=<hex>`
/// or `string=<hex>`.
fn generic(description: &str, is_request: bool) -> (Option<TaskId>, String) {
    let mut parent = None;
    let mut text = String::new();
    for part in description.split(';') {
        let (k, v) = part.split_once('=').unwrap();
        match k {
            "parent" => {
                parent = Some(match v.split_once(':') {
                    None => TaskId::EMPTY,
                    Some((n, id)) => TaskId {
                        task: Some((n.to_owned(), id.parse().unwrap())),
                    },
                });
            }
            "string" => text = unhex(v),
            other => panic!("unknown part {other}"),
        }
    }
    assert_eq!(parent.is_some(), is_request);
    (parent, text)
}

type Writer = Box<dyn Fn(&mut Output<'_>) -> Result<(), WireError>>;
type Check = Box<dyn Fn(&[u8])>;

/// What a row's frame carries: the meta, the variable header, a writer for
/// its message, and a check of a decoded message.
fn expectation(row: &[String]) -> (FrameMeta, VariableHeader, Writer, Check) {
    let kind = row[0].as_str();
    let request_id: i64 = row[1].parse().unwrap();
    let compress = row[2] == "true";
    let last = row.len() - 1;
    let shell = WireVersion::SHELL;
    let meta = |status, version| FrameMeta {
        request_id,
        status,
        version,
    };
    match kind {
        "tcp_handshake_request" => {
            let message = TcpHandshakeRequest {
                parent: TaskId::EMPTY,
                version: Some(WireVersion::MIN_COMPAT),
            };
            let check = message.clone();
            (
                meta(Status::request(compress, true), WireVersion::MIN_COMPAT),
                variable(row, true),
                Box::new(move |o| message.write(o)),
                Box::new(move |m| {
                    let mut input = Input::new(m);
                    assert_eq!(TcpHandshakeRequest::read(&mut input).unwrap(), check);
                }),
            )
        }
        "tcp_handshake_response" => {
            let message = TcpHandshakeResponse { version: shell };
            (
                meta(Status::response(compress, true, false), shell),
                variable(row, false),
                Box::new(move |o| message.write(o)),
                Box::new(move |m| {
                    assert_eq!(
                        TcpHandshakeResponse::read(&mut Input::new(m)).unwrap(),
                        message
                    );
                }),
            )
        }
        "service_handshake_request" => (
            meta(Status::request(compress, false), shell),
            variable(row, true),
            Box::new(|o| TransportHandshakeRequest::default().write(o)),
            Box::new(|m| {
                assert_eq!(
                    TransportHandshakeRequest::read(&mut Input::new(m)).unwrap(),
                    TransportHandshakeRequest::default()
                );
            }),
        ),
        "service_handshake_response" => {
            let n = node(&row[6..17]);
            let message = TransportHandshakeResponse {
                version: n.version,
                node: Some(n),
                cluster_name: "serverless".to_owned(),
            };
            let check = message.clone();
            (
                meta(Status::response(compress, false, false), shell),
                variable(row, false),
                Box::new(move |o| message.write(o)),
                Box::new(move |m| {
                    assert_eq!(
                        TransportHandshakeResponse::read(&mut Input::new(m)).unwrap(),
                        check
                    );
                }),
            )
        }
        "request" | "response" => {
            let is_request = kind == "request";
            let (parent, text) = generic(&row[6], is_request);
            let (p2, t2) = (parent.clone(), text.clone());
            let status = if is_request {
                Status::request(compress, false)
            } else {
                Status::response(compress, false, false)
            };
            assert_eq!(last, 7);
            (
                meta(status, shell),
                variable(row, is_request),
                Box::new(move |o| {
                    if let Some(parent) = &parent {
                        parent.write(o)?;
                    }
                    o.string(&text)
                }),
                Box::new(move |m| {
                    let mut input = Input::new(m);
                    if let Some(parent) = &p2 {
                        assert_eq!(&TaskId::read(&mut input).unwrap(), parent);
                    }
                    assert_eq!(input.string("s").unwrap(), t2);
                    input.finish("m").unwrap();
                }),
            )
        }
        other => panic!("unknown row kind {other}"),
    }
}

#[test]
fn every_recorded_frame_encodes_and_decodes_as_the_javas() {
    let rows = rows("frames.txt");
    assert_eq!(rows.len(), 22);
    for row in &rows {
        let expected = bytes(row.last().unwrap());
        let (meta, variable, write, check) = expectation(row);
        let mut buf = Vec::new();
        frame::encode(&mut buf, MAX, meta, &variable, &write).unwrap();
        let compressed = meta.status.is_compressed();
        if !compressed {
            assert_eq!(hex(&buf), hex(&expected), "{} {}", row[0], row[1]);
        }
        // Both the Java's frame and ours decode to the same fields and message.
        for encoded in [&expected, &buf] {
            let decoded = frame::decode(encoded, MAX).unwrap();
            assert_eq!(decoded.header.request_id, meta.request_id);
            assert_eq!(decoded.header.status, meta.status);
            assert_eq!(decoded.header.version, meta.version);
            assert_eq!(decoded.variable, variable);
            check(&decoded.message);
        }
        assert_eq!(&expected[..2], b"ES");
    }
}

#[test]
fn a_ping_and_bad_prefixes() {
    assert_eq!(Prefix::read(&PING, MAX), Ok(Prefix::Ping));
    assert!(matches!(
        Prefix::read(b"EX\0\0\0\x20", MAX),
        Err(WireError::Malformed { .. })
    ));
    // Shorter than the counted fixed header (17 bytes), negative, past the bound.
    assert!(matches!(
        Prefix::read(b"ES\0\0\0\x10", MAX),
        Err(WireError::Malformed { .. })
    ));
    assert_eq!(
        Prefix::read(b"ES\0\0\0\x11", MAX),
        Ok(Prefix::Message { rest: 17 })
    );
    assert!(matches!(
        Prefix::read(b"ES\xff\xff\xff\xfe", MAX),
        Err(WireError::Malformed { .. })
    ));
    assert_eq!(
        Prefix::read(b"ES\0\0\x01\0", 262),
        Ok(Prefix::Message { rest: 256 })
    );
    assert!(matches!(
        Prefix::read(b"ES\0\0\x01\0", 261),
        Err(WireError::TooLarge {
            limit: 261,
            actual: 262,
            ..
        })
    ));
    let mut fixed = [0u8; FIXED_HEADER_BYTES];
    fixed[..6].copy_from_slice(&PING);
    assert!(matches!(
        FixedHeader::read(&fixed, MAX),
        Err(WireError::Malformed { .. })
    ));
}

/// A recorded uncompressed request frame, as bytes.
fn recorded_request() -> Vec<u8> {
    let rows = rows("frames.txt");
    let row = rows
        .iter()
        .find(|r| r[0] == "request" && r[2] == "false")
        .unwrap();
    bytes(row.last().unwrap())
}

#[test]
fn variable_header_lengths_are_checked() {
    let frame_bytes = recorded_request();
    let mut fixed = [0u8; FIXED_HEADER_BYTES];
    fixed.copy_from_slice(&frame_bytes[..FIXED_HEADER_BYTES]);
    let header = FixedHeader::read(&fixed, MAX).unwrap();
    let total = header.variable_header_bytes + header.message_bytes;
    // Longer than the frame.
    let mut long = fixed;
    long[19..23].copy_from_slice(&i32::try_from(total + 1).unwrap().to_be_bytes());
    assert!(matches!(
        FixedHeader::read(&long, MAX),
        Err(WireError::Malformed { .. })
    ));
    // Negative.
    long[19..23].copy_from_slice(&(-1i32).to_be_bytes());
    assert!(matches!(
        FixedHeader::read(&long, MAX),
        Err(WireError::Malformed { .. })
    ));
    // Past 64 KiB, in a frame long enough to hold it.
    let mut big = fixed;
    big[2..6].copy_from_slice(&(17i32 + 70_000).to_be_bytes());
    big[19..23].copy_from_slice(&65_537i32.to_be_bytes());
    assert!(matches!(
        FixedHeader::read(&big, MAX),
        Err(WireError::TooLarge { .. })
    ));
    big[19..23].copy_from_slice(&65_536i32.to_be_bytes());
    assert_eq!(
        FixedHeader::read(&big, MAX).unwrap().message_bytes,
        70_000 - 65_536
    );
    // A frame shorter or longer than its length, and one cut in its header.
    assert!(frame::decode(&frame_bytes[..frame_bytes.len() - 1], MAX).is_err());
    let mut extra = frame_bytes.clone();
    extra.push(0);
    assert!(frame::decode(&extra, MAX).is_err());
    assert!(matches!(
        frame::decode(&frame_bytes[..22], MAX),
        Err(WireError::Truncated { .. })
    ));
    // A variable header with bytes after its action.
    let decoded = frame::decode(&frame_bytes, MAX).unwrap();
    let mut out = Vec::new();
    decoded
        .variable
        .write(&mut Output::new(&mut out, MAX), true)
        .unwrap();
    out.push(0);
    assert!(matches!(
        VariableHeader::read(&out, true),
        Err(WireError::Malformed { .. })
    ));
    assert_eq!(
        decoded
            .variable
            .header("x-serverless-transport-mac")
            .map(str::len),
        Some(116)
    );
    assert_eq!(decoded.variable.header("absent"), None);
}

#[test]
fn compressed_messages_inflate_to_a_bound() {
    let message = vec![b'a'; 100_000];
    let compressed = frame::deflate(&message);
    assert!(compressed.len() < 1_000);
    assert_eq!(&compressed[..4], b"DFL\0");
    assert_eq!(
        frame::inflate_bounded(&compressed, 100_000).unwrap(),
        message
    );
    assert!(matches!(
        frame::inflate_bounded(&compressed, 99_999),
        Err(WireError::TooLarge { limit: 99_999, .. })
    ));
    // No marker, a cut stream, bytes after the stream, garbage.
    assert!(frame::inflate_bounded(&compressed[1..], MAX).is_err());
    assert!(frame::inflate_bounded(&compressed[..compressed.len() - 3], MAX).is_err());
    let mut trailing = compressed.clone();
    trailing.push(0);
    assert!(frame::inflate_bounded(&trailing, MAX).is_err());
    assert!(frame::inflate_bounded(b"DFL\0\xff\xff\xff", MAX).is_err());
    assert_eq!(
        frame::inflate_bounded(&frame::deflate(b""), 0).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn a_frame_past_its_limit_leaves_the_buffer_as_it_was() {
    let mut buf = vec![7u8];
    let meta = FrameMeta {
        request_id: 1,
        status: Status::request(false, false),
        version: WireVersion::SHELL,
    };
    let variable = VariableHeader {
        action: TRANSPORT_HANDSHAKE_ACTION.to_owned(),
        ..VariableHeader::default()
    };
    let result = frame::encode(&mut buf, 40, meta, &variable, |o| o.bytes(&[0u8; 64]));
    assert!(matches!(result, Err(WireError::TooLarge { .. })));
    assert_eq!(buf, vec![7]);
    frame::encode(&mut buf, 100, meta, &variable, |o| o.byte(0)).unwrap();
    assert_eq!(buf[0], 7);
    assert_eq!(
        frame::decode(&buf[1..], MAX).unwrap().variable.action,
        TRANSPORT_HANDSHAKE_ACTION
    );
}

#[test]
fn status_bits_are_the_javas() {
    assert_eq!(Status::request(false, false).0, 0);
    assert_eq!(Status::request(true, true).0, 0b1100);
    assert_eq!(Status::response(false, false, true).0, 0b0011);
    let s = Status::response(true, true, false);
    assert!(!s.is_request() && s.is_compressed() && s.is_handshake() && !s.is_error());
    assert!(Status::response(false, false, true).is_error());
    let plain = Status::request(false, false);
    assert!(plain.is_request() && !plain.is_handshake() && !plain.is_compressed());
}

#[test]
fn a_tcp_handshake_request_may_end_before_its_version_and_ignores_what_follows_it() {
    let mut empty = Input::new(&[0]);
    assert_eq!(TcpHandshakeRequest::read(&mut empty).unwrap().version, None);
    let mut buf = Vec::new();
    TcpHandshakeRequest {
        parent: TaskId::EMPTY,
        version: None,
    }
    .write(&mut Output::new(&mut buf, 16))
    .unwrap();
    assert_eq!(buf, [0]);
    // The byte reference holds the version and a byte a later version added.
    let mut longer = Input::new(&[0, 5, 0xc3, 0x9b, 0xbd, 0x41, 0x2a]);
    assert_eq!(
        TcpHandshakeRequest::read(&mut longer).unwrap().version,
        Some(WireVersion::SHELL)
    );
    let mut trailing = Input::new(&[0, 4, 0xc3, 0x9b, 0xbd, 0x41, 0x2a]);
    assert!(TcpHandshakeRequest::read(&mut trailing).is_err());
    assert_eq!(TCP_HANDSHAKE_ACTION, "internal:tcp/handshake");
}

#[test]
fn a_transport_handshake_response_may_name_no_node() {
    let message = TransportHandshakeResponse {
        node: None,
        cluster_name: "c".to_owned(),
        version: WireVersion::SHELL,
    };
    let mut buf = Vec::new();
    message.write(&mut Output::new(&mut buf, 64)).unwrap();
    assert_eq!(
        TransportHandshakeResponse::read(&mut Input::new(&buf)).unwrap(),
        message
    );
}

#[test]
fn a_variable_header_no_reader_accepts_is_not_written_and_a_broken_one_not_read() {
    let meta = FrameMeta {
        request_id: 1,
        status: Status::request(false, false),
        version: WireVersion::SHELL,
    };
    let variable = VariableHeader {
        request_headers: vec![("k".to_owned(), "v".repeat(70_000))],
        action: "a".to_owned(),
        ..VariableHeader::default()
    };
    let mut buf = Vec::new();
    assert!(matches!(
        frame::encode(&mut buf, MAX, meta, &variable, |_| Ok(())),
        Err(WireError::TooLarge { limit: 65_536, .. })
    ));
    assert!(buf.is_empty());
    // A message writer's error is returned and nothing is left behind.
    let failing = frame::encode(&mut buf, MAX, meta, &VariableHeader::default(), |_| {
        Err(WireError::Truncated { what: "m" })
    });
    assert_eq!(failing, Err(WireError::Truncated { what: "m" }));
    assert!(buf.is_empty());
    // A request frame whose variable header is cut short of its action.
    let mut frame_bytes = recorded_request();
    let mut fixed = [0u8; FIXED_HEADER_BYTES];
    fixed.copy_from_slice(&frame_bytes[..FIXED_HEADER_BYTES]);
    let header = FixedHeader::read(&fixed, MAX).unwrap();
    let action_len_at = FIXED_HEADER_BYTES + header.variable_header_bytes - 35;
    frame_bytes[action_len_at] = 0x7f;
    assert!(frame::decode(&frame_bytes, MAX).is_err());
}

#[test]
fn a_transport_handshake_request_carries_its_parent() {
    let request = TransportHandshakeRequest {
        parent: TaskId {
            task: Some(("w1".to_owned(), 3)),
        },
    };
    let mut buf = Vec::new();
    request.write(&mut Output::new(&mut buf, 64)).unwrap();
    assert_eq!(
        TransportHandshakeRequest::read(&mut Input::new(&buf)).unwrap(),
        request
    );
    buf.push(0);
    assert!(TransportHandshakeRequest::read(&mut Input::new(&buf)).is_err());
}

#[test]
fn a_variable_header_of_exactly_64_kib_is_written() {
    let meta = FrameMeta {
        request_id: 1,
        status: Status::response(false, false, false),
        version: WireVersion::SHELL,
    };
    // Map count (1 byte), "k" (2), the value's char count (3 bytes past
    // 16,383), the value, the response-header count (1).
    let value_len = 65_536 - 1 - 2 - 3 - 1;
    let variable = VariableHeader {
        request_headers: vec![("k".to_owned(), "v".repeat(value_len))],
        ..VariableHeader::default()
    };
    let mut buf = Vec::new();
    frame::encode(&mut buf, MAX, meta, &variable, |_| Ok(())).unwrap();
    let decoded = frame::decode(&buf, MAX).unwrap();
    assert_eq!(decoded.header.variable_header_bytes, 65_536);
}
