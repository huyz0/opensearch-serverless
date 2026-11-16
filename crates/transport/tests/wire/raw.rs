//! The server against frames written by hand, as a misbehaving or older
//! peer would send them: the order of handshakes, versions, pings, the
//! channel cap and timeouts.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_transport::error::WireError;
use oss_transport::exception::RemoteException;
use oss_transport::frame::{self, FrameMeta, PING, Status, VariableHeader};
use oss_transport::handshake::{TCP_HANDSHAKE_ACTION, TcpHandshakeRequest};
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output, TaskId};
use oss_transport::version::WireVersion;

use crate::support::{MIB, budget, config, eventually, handlers, serve};

/// A raw socket that writes frames by hand.
pub(crate) fn raw(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
}

pub(crate) fn send(
    stream: &mut TcpStream,
    meta: FrameMeta,
    action: &str,
    write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>,
) {
    let variable = VariableHeader {
        action: action.into(),
        ..VariableHeader::default()
    };
    let mut buf = Vec::new();
    frame::encode(&mut buf, MIB, meta, &variable, write).unwrap();
    stream.write_all(&buf).unwrap();
}

/// Reads one frame (or ping) back.
pub(crate) fn receive(stream: &mut TcpStream) -> Vec<u8> {
    let mut prefix = [0u8; 6];
    stream.read_exact(&mut prefix).unwrap();
    let len = i32::from_be_bytes([prefix[2], prefix[3], prefix[4], prefix[5]]);
    let mut bytes = prefix.to_vec();
    if len >= 0 {
        let mut rest = vec![0u8; len as usize];
        stream.read_exact(&mut rest).unwrap();
        bytes.extend_from_slice(&rest);
    }
    bytes
}

fn tcp_handshake(stream: &mut TcpStream) {
    let request = TcpHandshakeRequest {
        parent: TaskId::EMPTY,
        version: Some(WireVersion::MIN_COMPAT),
    };
    send(
        stream,
        FrameMeta {
            request_id: 1,
            status: Status::request(false, true),
            version: WireVersion::MIN_COMPAT,
        },
        TCP_HANDSHAKE_ACTION,
        |o| request.write(o),
    );
    let answer = receive(stream);
    let frame = frame::decode(&answer, MIB).unwrap();
    assert!(frame.header.status.is_handshake());
    assert_eq!(
        frame.header.version,
        WireVersion::MIN_COMPAT,
        "the request's version back"
    );
}

pub(crate) fn closed(stream: &mut TcpStream) -> bool {
    let mut byte = [0u8; 1];
    matches!(stream.read(&mut byte), Ok(0) | Err(_))
}

#[test]
fn a_request_without_a_tcp_handshake_is_served_and_a_ping_is_echoed() {
    // A Java peer makes the TCP handshake on its connection's first channel
    // only, and sends requests on the others without one.
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    stream.write_all(&PING).unwrap();
    assert_eq!(receive(&mut stream), PING);
    let shell = FrameMeta {
        request_id: 5,
        status: Status::request(false, false),
        version: WireVersion::SHELL,
    };
    send(&mut stream, shell, "test:echo", |o| {
        TaskId::EMPTY.write(o)?;
        o.bytes(b"hi")
    });
    let answer = receive(&mut stream);
    let frame = frame::decode(&answer, MIB).unwrap();
    assert_eq!(frame.header.request_id, 5);
    assert!(!frame.header.status.is_error());
    assert_eq!(&*frame.message, b"hi");
    tcp_handshake(&mut stream);
}

#[test]
fn a_request_at_another_version_is_refused() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    tcp_handshake(&mut stream);
    send(
        &mut stream,
        FrameMeta {
            request_id: 9,
            status: Status::request(false, false),
            version: WireVersion::MIN_COMPAT,
        },
        "test:echo",
        |o| TaskId::EMPTY.write(o),
    );
    let answer = receive(&mut stream);
    let frame = frame::decode(&answer, MIB).unwrap();
    assert!(frame.header.status.is_error());
    assert_eq!(frame.header.version, WireVersion::SHELL);
    let failure = RemoteException::read(&mut Input::new(&frame.message))
        .unwrap()
        .unwrap();
    assert_eq!(failure.http_status(), 400);
}

#[test]
fn a_tcp_handshake_from_an_unsupported_version_closes_the_channel() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    let old = WireVersion::from_id(2_180_099);
    let request = TcpHandshakeRequest {
        parent: TaskId::EMPTY,
        version: Some(old),
    };
    send(
        &mut stream,
        FrameMeta {
            request_id: 1,
            status: Status::request(false, true),
            version: old,
        },
        TCP_HANDSHAKE_ACTION,
        |o| request.write(o),
    );
    assert!(closed(&mut stream));
}

#[test]
fn channels_past_the_cap_are_closed_and_silent_ones_time_out() {
    let budget = budget();
    let mut config = config();
    config.max_inbound_channels = 1;
    config.handshake_timeout = Duration::from_millis(200);
    config.idle_timeout = Duration::from_millis(300);
    let server = serve(config, &budget, handlers(vec![]));
    let mut first = raw(server.address());
    tcp_handshake(&mut first);
    let mut second = raw(server.address());
    assert!(closed(&mut second), "past the cap");
    let started = Instant::now();
    assert!(closed(&mut first), "idle past the timeout");
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(eventually(|| server.channels() == 0));
    let mut silent = raw(server.address());
    let started = Instant::now();
    assert!(closed(&mut silent), "no handshake in time");
    assert!(started.elapsed() >= Duration::from_millis(150));
}

#[test]
fn a_garbled_frame_closes_the_channel() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    stream.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
    assert!(closed(&mut stream));
}

/// Writes `self.0` bytes, at most the frame bound, so the response frame
/// passes it.
struct Fill(usize);

impl Handler for Fill {
    fn handle(&self, _: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        out.bytes(&vec![0u8; self.0])
            .map_err(|_| RemoteException::status("big", "INTERNAL_SERVER_ERROR"))
    }
}

#[test]
fn a_response_past_the_frame_bound_is_answered_as_an_error() {
    let budget = budget();
    let mut config = config();
    config.max_frame_bytes = 4096;
    let server = serve(
        config,
        &budget,
        handlers(vec![("test:fill", Arc::new(Fill(4096)))]),
    );
    let mut stream = raw(server.address());
    tcp_handshake(&mut stream);
    let shell = FrameMeta {
        request_id: 3,
        status: Status::request(false, false),
        version: WireVersion::SHELL,
    };
    send(&mut stream, shell, "test:fill", |o| TaskId::EMPTY.write(o));
    let answer = receive(&mut stream);
    let frame = frame::decode(&answer, MIB).unwrap();
    assert!(frame.header.status.is_error());
    let failure = RemoteException::read(&mut Input::new(&frame.message))
        .unwrap()
        .unwrap();
    assert_eq!(failure.http_status(), 500);
    assert_eq!(
        failure.root().message.as_deref(),
        Some("the response is larger than a transport frame")
    );
    assert!(format!("{server:?}").contains("Server"));
}

#[test]
fn a_response_on_an_inbound_channel_closes_it() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    tcp_handshake(&mut stream);
    send(
        &mut stream,
        FrameMeta {
            request_id: 4,
            status: Status::response(false, false, false),
            version: WireVersion::SHELL,
        },
        "",
        |_| Ok(()),
    );
    assert!(closed(&mut stream));
    assert!(eventually(|| server.channels() == 0));
}

#[test]
fn a_frame_cut_short_closes_the_channel() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    stream.write_all(b"ES\0").unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(closed(&mut stream));
    assert!(eventually(|| server.channels() == 0));
}

#[test]
fn a_compressed_transport_handshake_is_answered_compressed() {
    // A Java peer with `transport.compress` compresses the transport
    // handshake, whose answer the channel's own thread writes.
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    tcp_handshake(&mut stream);
    send(
        &mut stream,
        FrameMeta {
            request_id: 2,
            status: Status::request(true, false),
            version: WireVersion::SHELL,
        },
        oss_transport::handshake::TRANSPORT_HANDSHAKE_ACTION,
        |o| TaskId::EMPTY.write(o),
    );
    let answer = receive(&mut stream);
    let frame = frame::decode(&answer, MIB).unwrap();
    assert!(frame.header.status.is_compressed());
    let described =
        oss_transport::handshake::TransportHandshakeResponse::read(&mut Input::new(&frame.message))
            .unwrap();
    assert_eq!(described.node.unwrap().ephemeral_id, "eph-1");
}

#[test]
fn a_tcp_handshake_from_the_shells_own_version_is_accepted() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let mut stream = raw(server.address());
    let request = TcpHandshakeRequest {
        parent: TaskId::EMPTY,
        version: Some(WireVersion::SHELL),
    };
    send(
        &mut stream,
        FrameMeta {
            request_id: 1,
            status: Status::request(false, true),
            version: WireVersion::SHELL,
        },
        TCP_HANDSHAKE_ACTION,
        |o| request.write(o),
    );
    let answer = receive(&mut stream);
    let frame = frame::decode(&answer, MIB).unwrap();
    assert!(frame.header.status.is_handshake());
    assert!(!frame.header.status.is_error());
}
