//! What the socket tests share: a budget, a node, handlers, a server and
//! client calls.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_transport::client::{Call, CallError, Client, PeerId};
use oss_transport::exception::RemoteException;
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handler, Handlers, Request, Server, ServerConfig};
use oss_transport::stream::{Input, Output, TaskId};
use oss_transport::version::WireVersion;

pub(crate) const MIB: usize = 1024 * 1024;

pub(crate) fn budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 * MIB)
            .with_pool(Pool::Transport, 64 * MIB)
            .unwrap(),
    )
}

pub(crate) fn node(ephemeral: &str) -> DiscoveryNode {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    DiscoveryNode {
        name: "w1".into(),
        id: "node-1".into(),
        ephemeral_id: ephemeral.into(),
        host_name: "127.0.0.1".into(),
        host_address: "127.0.0.1".into(),
        address: TransportAddress {
            ip,
            host: "127.0.0.1".into(),
            port: 9300,
        },
        stream_address: None,
        attributes: vec![("zone".into(), "a".into())],
        roles: vec![NodeRole::data()],
        version: WireVersion::SHELL,
    }
}

/// Answers the message after the parent task id, as sent.
pub(crate) struct Echo;

impl Handler for Echo {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let mut input = Input::new(request.message);
        TaskId::read(&mut input).map_err(|_| RemoteException::status("bad", "BAD_REQUEST"))?;
        out.bytes(input.rest())
            .map_err(|_| RemoteException::status("big", "INTERNAL_SERVER_ERROR"))
    }
}

/// Refuses every request's authentication.
pub(crate) struct Forbid;

impl Handler for Forbid {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        Err(RemoteException::security("bad mac", "FORBIDDEN"))
    }
}

/// Answers a status failure that is not about authentication.
pub(crate) struct Unavailable;

impl Handler for Unavailable {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        Err(RemoteException::status("not now", "SERVICE_UNAVAILABLE"))
    }
}

/// Waits on a barrier before answering, so a test holds the workers.
pub(crate) struct Hold(pub(crate) Arc<Barrier>, pub(crate) Arc<AtomicUsize>);

impl Handler for Hold {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.wait();
        Ok(())
    }
}

/// Sleeps before answering.
pub(crate) struct Slow(pub(crate) Duration);

impl Handler for Slow {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        thread::sleep(self.0);
        Ok(())
    }
}

pub(crate) fn handlers(extra: Vec<(&str, Arc<dyn Handler>)>) -> Handlers {
    let mut map: Handlers = HashMap::new();
    map.insert("test:echo".into(), Arc::new(Echo));
    for (action, handler) in extra {
        map.insert(action.into(), handler);
    }
    map
}

pub(crate) fn serve(config: ServerConfig, budget: &MemoryBudget, handlers: Handlers) -> Server {
    Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        config,
        budget.clone(),
        handlers,
    )
    .unwrap()
}

pub(crate) fn config() -> ServerConfig {
    let mut config = ServerConfig::new(node("eph-1"), "cluster", 4);
    config.max_frame_bytes = MIB;
    config
}

pub(crate) fn peer(server: &Server) -> PeerId {
    PeerId {
        address: server.address(),
        ephemeral_id: Some("eph-1".into()),
    }
}

pub(crate) fn call(action: &str, compress: bool, millis: u64) -> Call<'static> {
    let action: &'static str = Box::leak(action.to_owned().into_boxed_str());
    Call {
        action,
        headers: &[],
        compress,
        deadline: Instant::now() + Duration::from_millis(millis),
    }
}

pub(crate) fn echo(
    client: &Client,
    peer: &PeerId,
    payload: &[u8],
    compress: bool,
) -> Result<Vec<u8>, CallError> {
    client
        .call(peer, call("test:echo", compress, 5_000), |o| {
            o.bytes(payload)
        })
        .map(|a| a.message().to_vec())
}

pub(crate) fn remote(result: Result<Vec<u8>, CallError>) -> RemoteException {
    match result {
        Err(CallError::Remote(e)) => e,
        other => panic!("expected a remote failure, got {other:?}"),
    }
}

/// Waits up to two seconds for `cond`.
pub(crate) fn eventually(mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    cond()
}

/// Answers the value of request header `x-test`.
pub(crate) struct HeaderEcho;

impl Handler for HeaderEcho {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let value = request.variable.header("x-test").unwrap_or("none");
        out.string(value)
            .map_err(|_| RemoteException::status("big", "INTERNAL_SERVER_ERROR"))
    }
}

/// Answers a 403 that is not a security failure.
pub(crate) struct StatusForbidden;

impl Handler for StatusForbidden {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        Err(RemoteException::status("no", "FORBIDDEN"))
    }
}

/// Answers a security failure that is not a 401 or 403.
pub(crate) struct SecurityUnavailable;

impl Handler for SecurityUnavailable {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        Err(RemoteException::security("later", "SERVICE_UNAVAILABLE"))
    }
}
