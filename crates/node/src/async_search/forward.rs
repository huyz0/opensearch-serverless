//! A read or delete of another node's search over the transport: the
//! owner's handler, and the send side. The node is found in the leases this
//! node already holds, so a relay makes no request to the store.

use std::sync::Arc;
use std::time::Duration;

use oss_transport::actions::async_search::{
    ASYNC_SEARCH_ACTION, AsyncSearchRequest, AsyncSearchResponse, Op as WireOp,
};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use super::{Call, Op, Relay, RelayError, Service};
use crate::forward::ForwardError;
use crate::forward_call::{owner_of, signed_call};
use crate::jobs::Who;
use crate::push_link::Leases;

/// How long a relayed call waits for the node that runs the search: the
/// longest wait a read may ask for, and a margin.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Answers relayed calls from this node's own searches. It never relays
/// again: a call names a number on this node.
pub struct AsyncSearchHandler {
    service: Arc<Service>,
}

impl AsyncSearchHandler {
    /// A handler answering from `service`.
    #[must_use]
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }
}

impl Handler for AsyncSearchHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let wire = AsyncSearchRequest::read(&mut Input::new(request.message)).map_err(|e| {
            // ALLOC: error path; one message.
            RemoteException::status(
                &format!("bad asynchronous search request: {e}"),
                "BAD_REQUEST",
            )
        })?;
        let reply = self.service.answer(&Call {
            op: match wire.op {
                WireOp::Get => Op::Get,
                WireOp::Delete => Op::Delete,
            },
            number: wire.number,
            who: Who {
                tenant: wire.tenant,
                user: wire.user,
            },
            wait_ms: wire.wait_ms,
            keep_alive_ms: wire.keep_alive_ms,
        });
        AsyncSearchResponse {
            status: reply.status,
            // ALLOC: one answer, at most the result cap and its envelope.
            body: String::from_utf8_lossy(&reply.body).into_owned(),
        }
        .write(out)
        .map_err(|_| {
            RemoteException::status("the answer is too large to send", "INTERNAL_SERVER_ERROR")
        })
    }
}

/// Sends relayed calls over the transport client, signed.
pub struct AsyncSearchForwarder<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
}

impl<L: Leases> AsyncSearchForwarder<L> {
    /// A forwarder calling through `client`, signing with `auth`, finding
    /// nodes in `leases`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, leases: L) -> Self {
        Self {
            client,
            auth,
            leases,
        }
    }
}

impl<L: Leases> Relay for AsyncSearchForwarder<L> {
    fn call(&self, node: &str, call: &Call) -> Result<(u16, Vec<u8>), RelayError> {
        let (lease, address) = owner_of(&self.leases, node).map_err(|e| match e {
            ForwardError::NoPeer => RelayError::NoNode,
            _ => RelayError::Failed,
        })?;
        let wire = AsyncSearchRequest {
            op: match call.op {
                Op::Get => WireOp::Get,
                Op::Delete => WireOp::Delete,
            },
            number: call.number,
            // ALLOC: one caller per relayed call.
            tenant: call.who.tenant.clone(),
            user: call.who.user.clone(),
            wait_ms: call.wait_ms,
            keep_alive_ms: call.keep_alive_ms,
        };
        let answer = signed_call(
            &self.client,
            &self.auth,
            ASYNC_SEARCH_ACTION,
            (&lease, address),
            TIMEOUT,
            64,
            |out| wire.write(out),
        )
        .map_err(|_| RelayError::Failed)?;
        let reply = AsyncSearchResponse::read(&mut Input::new(answer.message()))
            .map_err(|_| RelayError::Failed)?;
        Ok((reply.status, reply.body.into_bytes()))
    }
}
