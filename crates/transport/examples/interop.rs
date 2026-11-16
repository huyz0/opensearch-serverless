//! The Rust half of the interop check against a real OpenSearch transport
//! (`fixtures/transport/TransportInterop.java`; README.md there says how to
//! run both).
//!
//! `interop serve`: serves `interop:echo` and `interop:fail` on loopback,
//! prints the port, and runs until stdin closes.
//! `interop call <port> <ephemeral id>`: calls the Java node there and
//! checks each answer; exits non-zero on the first mismatch.

#![allow(clippy::disallowed_methods)]
#![allow(clippy::print_stdout)]
// ARITH: a deadline ten seconds from now; it cannot overflow an Instant.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_transport::client::{Call, CallError, Client, ClientConfig, PeerId};
use oss_transport::exception::{ExceptionKind, RemoteException, ids};
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handler, Handlers, Request, Server, ServerConfig};
use oss_transport::stream::{Input, Output, TaskId};
use oss_transport::version::WireVersion;

const MIB: usize = 1024 * 1024;

/// Answers the byte reference after the parent task id.
struct Echo;

impl Handler for Echo {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let mut input = Input::new(request.message);
        let bad = |_| RemoteException::status("bad request", "BAD_REQUEST");
        TaskId::read(&mut input).map_err(bad)?;
        let payload = input.bytes_reference("a payload").map_err(bad)?;
        out.bytes_reference(payload)
            .map_err(|_| RemoteException::status("too big", "INTERNAL_SERVER_ERROR"))
    }
}

struct Fail;

impl Handler for Fail {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        Err(RemoteException::status(
            "rust says not now",
            "SERVICE_UNAVAILABLE",
        ))
    }
}

fn budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 * MIB)
            .with_pool(Pool::Transport, 64 * MIB)
            .expect("fits"),
    )
}

fn serve() {
    let mut handlers: Handlers = HashMap::new();
    handlers.insert("interop:echo".into(), Arc::new(Echo));
    handlers.insert("interop:fail".into(), Arc::new(Fail));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let node = DiscoveryNode {
        name: "w1".into(),
        id: "node-1".into(),
        ephemeral_id: "eph-1".into(),
        host_name: "127.0.0.1".into(),
        host_address: "127.0.0.1".into(),
        address: TransportAddress {
            ip,
            host: "127.0.0.1".into(),
            port: i32::from(port),
        },
        stream_address: None,
        attributes: vec![("zone".into(), "a".into())],
        roles: vec![NodeRole::data()],
        version: WireVersion::SHELL,
    };
    let server = Server::bind(
        SocketAddr::new(ip, port),
        ServerConfig::new(node, "interop", 4),
        budget(),
        handlers,
    )
    .expect("bind");
    println!("{} eph-1", server.address().port());
    let mut sink = [0u8; 64];
    while std::io::stdin().read(&mut sink).is_ok_and(|n| n > 0) {}
}

fn check(ok: bool, what: &str) {
    assert!(ok, "FAILED {what}");
    println!("ok {what}");
}

fn call(port: u16, ephemeral_id: &str) {
    let client = Client::new(ClientConfig::default(), budget());
    let peer = PeerId {
        address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ephemeral_id: Some(ephemeral_id.into()),
    };
    let request = |action: &'static str, compress: bool, text: &str| {
        let call = Call {
            action,
            headers: &[],
            compress,
            deadline: Instant::now() + Duration::from_secs(10),
        };
        client.call(&peer, call, |o| {
            TaskId::EMPTY.write(o)?;
            o.bytes_reference(text.as_bytes())
        })
    };
    for compress in [false, true] {
        let answer = request("interop:echo", compress, "hello from rust").expect("echo");
        let mut input = Input::new(answer.message());
        let payload = input.bytes_reference("a payload").expect("payload");
        check(
            payload == b"hello from rust",
            &format!("java echoed rust's request (compress={compress})"),
        );
    }
    match request("interop:missing", false, "x") {
        Err(CallError::Remote(e)) => check(
            e.kind == Some(ExceptionKind::OpenSearch(ids::REMOTE_TRANSPORT))
                && e.root().kind == Some(ExceptionKind::OpenSearch(ids::ACTION_NOT_FOUND))
                && e.root().message.as_deref() == Some("No handler for action [interop:missing]"),
            &format!("java's ActionNotFound reads as rust's: {e:?}"),
        ),
        other => check(false, &format!("java's ActionNotFound: {other:?}")),
    }
    match request("interop:fail", false, "x") {
        Err(CallError::Remote(e)) => check(
            e.http_status() == 503 && e.root().message.as_deref() == Some("java says not now"),
            &format!("java's status failure reads as rust's: {e:?}"),
        ),
        other => check(false, &format!("java's status failure: {other:?}")),
    }
    let impostor = PeerId {
        ephemeral_id: Some("eph-other".into()),
        ..peer.clone()
    };
    let refused = client.call(
        &impostor,
        Call {
            action: "interop:echo",
            headers: &[],
            compress: false,
            deadline: Instant::now() + Duration::from_secs(10),
        },
        |o| {
            TaskId::EMPTY.write(o)?;
            o.bytes_reference(b"x")
        },
    );
    check(
        matches!(refused, Err(CallError::WrongPeer(_))),
        "rust refuses the java node under another ephemeral id",
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => serve(),
        Some("call") => call(
            args.get(2).and_then(|p| p.parse().ok()).expect("port"),
            args.get(3).expect("ephemeral id"),
        ),
        _ => {
            println!("usage: interop serve | interop call <port> <ephemeral id>");
            std::process::exit(2);
        }
    }
}
