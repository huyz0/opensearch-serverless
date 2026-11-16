//! The client against a peer written by hand, for answers a real node
//! seldom sends: refused or garbled handshakes, pings and stray frames,
//! answers too large to hold, and a socket closed mid-call.

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use oss_transport::client::{Call, CallError, Client, ClientConfig, PeerId};
use oss_transport::exception::RemoteException;
use oss_transport::frame::{self, FrameMeta, PING, Status};
use oss_transport::handshake::{
    TCP_HANDSHAKE_ACTION, TRANSPORT_HANDSHAKE_ACTION, TcpHandshakeResponse,
    TransportHandshakeResponse,
};
use oss_transport::version::WireVersion;

use crate::raw::{receive, send};
use crate::support::{MIB, budget, config, echo, handlers, node, peer as peer_of, serve};

/// Serves one connection with `script`.
fn fake(script: impl FnOnce(TcpStream) + Send + 'static) -> PeerId {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        script(stream);
    });
    PeerId {
        address,
        ephemeral_id: None,
    }
}

fn request_id(stream: &mut TcpStream) -> i64 {
    frame::decode(&receive(stream), MIB)
        .unwrap()
        .header
        .request_id
}

fn answer(
    stream: &mut TcpStream,
    id: i64,
    handshake: bool,
    write: impl FnOnce(
        &mut oss_transport::stream::Output<'_>,
    ) -> Result<(), oss_transport::error::WireError>,
) {
    let meta = FrameMeta {
        request_id: id,
        status: Status::response(false, handshake, false),
        version: WireVersion::SHELL,
    };
    send(stream, meta, "", write);
}

fn fail(stream: &mut TcpStream, id: i64, failure: &RemoteException) {
    let meta = FrameMeta {
        request_id: id,
        status: Status::response(false, false, true),
        version: WireVersion::SHELL,
    };
    send(stream, meta, "", |o| failure.write(o));
}

/// Reads a request, checking its action, and returns its id.
fn expect(stream: &mut TcpStream, action: &str) -> i64 {
    let bytes = receive(stream);
    let frame = frame::decode(&bytes, MIB).unwrap();
    assert_eq!(frame.variable.action, action);
    frame.header.request_id
}

/// Answers both handshakes as a node at 3.10.0, after a stray response
/// the client must skip.
fn handshakes(stream: &mut TcpStream) {
    let id = expect(stream, TCP_HANDSHAKE_ACTION);
    answer(stream, id + 7, true, |o| o.bytes(b"stray"));
    answer(stream, id, true, |o| {
        TcpHandshakeResponse {
            version: WireVersion::SHELL,
        }
        .write(o)
    });
    let id = expect(stream, TRANSPORT_HANDSHAKE_ACTION);
    answer(stream, id, false, |o| {
        TransportHandshakeResponse {
            node: Some(node("eph-1")),
            cluster_name: "cluster".into(),
            version: WireVersion::SHELL,
        }
        .write(o)
    });
}

fn client() -> Client {
    Client::new(
        ClientConfig {
            max_frame_bytes: 4096,
            ..ClientConfig::default()
        },
        budget(),
    )
}

#[test]
fn a_refused_tcp_handshake_is_told_apart() {
    let peer = fake(|mut s| {
        let id = request_id(&mut s);
        fail(&mut s, id, &RemoteException::security("no", "UNAUTHORIZED"));
    });
    assert!(matches!(
        echo(&client(), &peer, b"x", false),
        Err(CallError::Unauthorized(_))
    ));
}

#[test]
fn a_peer_at_another_version_is_the_wrong_peer() {
    let peer = fake(|mut s| {
        let id = request_id(&mut s);
        answer(&mut s, id, true, |o| {
            TcpHandshakeResponse {
                version: WireVersion::MIN_COMPAT,
            }
            .write(o)
        });
    });
    assert!(matches!(
        echo(&client(), &peer, b"x", false),
        Err(CallError::WrongPeer(_))
    ));
}

#[test]
fn a_handshake_answered_with_garbage_or_nothing_fails_the_call() {
    let garbled = fake(|mut s| {
        let _ = request_id(&mut s);
        s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").unwrap();
    });
    assert!(matches!(
        echo(&client(), &garbled, b"x", false),
        Err(CallError::Io(std::io::ErrorKind::InvalidData))
    ));
    let silent = fake(|mut s| {
        let _ = request_id(&mut s);
        drop(s);
    });
    assert!(matches!(
        echo(&client(), &silent, b"x", false),
        Err(CallError::Io(_))
    ));
    let big = fake(|mut s| {
        let id = request_id(&mut s);
        answer(&mut s, id, true, |o| o.bytes(&[0u8; 8192]));
    });
    assert!(matches!(
        echo(&client(), &big, b"x", false),
        Err(CallError::TooLarge)
    ));
}

#[test]
fn pings_and_stray_frames_are_skipped_and_an_oversized_answer_fails_its_call() {
    let peer = fake(|mut s| {
        s.write_all(&PING).unwrap();
        handshakes(&mut s);
        let id = request_id(&mut s);
        s.write_all(&PING).unwrap();
        answer(&mut s, id + 100, false, |o| o.bytes(b"stray"));
        answer(&mut s, id, false, |o| o.bytes(&[0u8; 8192]));
        let id = request_id(&mut s);
        answer(&mut s, id, false, |o| o.bytes(b"fine"));
        let _ = request_id(&mut s);
        // Closed with the call in flight.
    });
    let client = client();
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::TooLarge)
    ));
    assert_eq!(echo(&client, &peer, b"x", false).unwrap(), b"fine");
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::Io(_))
    ));
    assert!(format!("{client:?}").contains("Client"));
}

#[test]
fn an_answer_more_than_memory_holds_is_busy() {
    use oss_mem::{BudgetConfig, MemoryBudget, Pool};
    let peer = fake(|mut s| {
        handshakes(&mut s);
        let id = request_id(&mut s);
        answer(&mut s, id, false, |o| o.bytes(&[0u8; 2048]));
        let _ = request_id(&mut s);
    });
    // Enough for the handshakes, not for the answer.
    let small = MemoryBudget::new(
        &BudgetConfig::new(1024)
            .with_pool(Pool::Transport, 1024)
            .unwrap(),
    );
    let client = Client::new(ClientConfig::default(), small);
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::Busy)
    ));
}

#[test]
fn a_socket_with_garbage_after_the_handshakes_fails_what_is_in_flight() {
    let peer = fake(|mut s| {
        handshakes(&mut s);
        let _ = request_id(&mut s);
        s.write_all(b"XX\0\0\0\0").unwrap();
        thread::sleep(Duration::from_millis(200));
    });
    assert!(matches!(
        echo(&client(), &peer, b"x", false),
        Err(CallError::Io(std::io::ErrorKind::InvalidData))
    ));
}

#[test]
fn a_peer_that_never_answers_its_handshake_times_out() {
    let peer = fake(|mut s| {
        let _ = request_id(&mut s);
        thread::sleep(Duration::from_millis(500));
    });
    let client = client();
    let call = Call {
        action: "test:echo",
        headers: &[],
        compress: false,
        deadline: Instant::now() + Duration::from_millis(150),
    };
    assert!(matches!(
        client.call(&peer, call, |_| Ok(())),
        Err(CallError::Timeout)
    ));
    assert_eq!(client.sockets(), 0);
}

#[test]
fn the_clients_socket_cap_makes_a_call_to_another_peer_busy() {
    let budget = budget();
    let first = serve(config(), &budget, handlers(vec![]));
    let second = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(
        ClientConfig {
            max_outbound_sockets: 1,
            ..ClientConfig::default()
        },
        budget.clone(),
    );
    assert_eq!(echo(&client, &peer_of(&first), b"x", false).unwrap(), b"x");
    let call = Call {
        action: "test:echo",
        headers: &[],
        compress: false,
        deadline: Instant::now() + Duration::from_millis(50),
    };
    let started = Instant::now();
    assert!(matches!(
        client.call(&peer_of(&second), call, |_| Ok(())),
        Err(CallError::Busy)
    ));
    assert!(
        started.elapsed() >= Duration::from_millis(40),
        "waits for a socket until the deadline"
    );
}

#[test]
fn a_call_waits_for_a_socket_another_call_is_opening() {
    let peer = fake(|mut s| {
        // A slow handshake keeps the only slot opening.
        thread::sleep(Duration::from_millis(150));
        handshakes(&mut s);
        for _ in 0..2 {
            let id = request_id(&mut s);
            answer(&mut s, id, false, |o| o.bytes(b"ok"));
        }
        thread::sleep(Duration::from_millis(100));
    });
    let client = std::sync::Arc::new(Client::new(
        ClientConfig {
            max_frame_bytes: 4096,
            connections_per_peer: 1,
            ..ClientConfig::default()
        },
        budget(),
    ));
    let calls: Vec<_> = (0..2)
        .map(|_| {
            let client = std::sync::Arc::clone(&client);
            let peer = peer.clone();
            thread::spawn(move || echo(&client, &peer, b"x", false))
        })
        .collect();
    for c in calls {
        assert_eq!(c.join().unwrap().unwrap(), b"ok");
    }
    assert_eq!(client.sockets(), 1);
}
