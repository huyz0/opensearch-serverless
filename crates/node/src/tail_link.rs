//! The writer's side of the tail's transport: the push to a reader and the
//! answer to a reader's fetch.
//!
//! The push is the send of the shell's `TailPush`, and the handler is its
//! fetch action
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`).
//! The push reads the leases the node already holds and makes no store
//! request; it is signed as every forwarded request is.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_transport::actions::tail::{FetchRequest, PUSH_ACTION, PushRequest, PushResponse};
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Call, Client, PeerId};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::push_link::Leases;
use crate::push_sender::LinkError;
use crate::tail_push::TailLink;
use crate::tail_serve::TailServe;

/// How long a push waits for a reader to take it.
pub const SEND_TIMEOUT: Duration = Duration::from_millis(500);

/// The largest push message.
pub const MAX_PUSH_BYTES: usize = 8 << 20;

/// The most a fetch request takes: names, a uuid, numbers.
pub const MAX_FETCH_BYTES: usize = 4096;

/// Sends tail pushes over the transport client, signed. A lease that is not
/// revoked is tried even if it has expired, as the commit push does: a push
/// to a node that is gone costs one timeout and is a lost push.
pub struct TransportTailLink<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
}

impl<L: Leases> TransportTailLink<L> {
    /// A link calling through `client`, signing with `auth`, finding peers
    /// in `leases`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, leases: L) -> Self {
        Self {
            client,
            auth,
            leases,
        }
    }
}

impl<L: Leases> TailLink for TransportTailLink<L> {
    fn send(&self, target: &str, request: &PushRequest) -> Result<PushResponse, LinkError> {
        let leases = self.leases.current();
        let lease = leases
            .iter()
            .find(|l| l.node_id == target && !l.revoked)
            .ok_or(LinkError)?;
        let address: SocketAddr = lease.address.parse().map_err(|_| LinkError)?;
        // ALLOC: one push message, at most MAX_PUSH_BYTES.
        let mut message = Vec::new();
        request
            .write(&mut Output::new(&mut message, MAX_PUSH_BYTES))
            .map_err(|_| LinkError)?;
        let mac = self
            .auth
            .sign(
                PUSH_ACTION,
                &message,
                &identity(&lease.node_id, &lease.ephemeral_id),
                &[],
            )
            .ok_or(LinkError)?;
        // ALLOC: the MAC header, one entry.
        let headers = [(MAC_HEADER.to_owned(), mac)];
        let peer = PeerId {
            address,
            // ALLOC: one ephemeral id a call.
            ephemeral_id: Some(lease.ephemeral_id.clone()),
        };
        let call = Call {
            action: PUSH_ACTION,
            headers: &headers,
            compress: false,
            deadline: Instant::now().checked_add(SEND_TIMEOUT).ok_or(LinkError)?,
        };
        let answer = self
            .client
            .call(&peer, call, |out| out.bytes(&message))
            .map_err(|_| LinkError)?;
        PushResponse::read(&mut Input::new(answer.message())).map_err(|_| LinkError)
    }
}

/// Answers `internal:serverless/tail/fetch` on a writer.
pub struct TailFetchHandler {
    serve: Arc<TailServe>,
}

impl TailFetchHandler {
    /// A handler answering from `serve`.
    pub fn new(serve: Arc<TailServe>) -> Self {
        Self { serve }
    }
}

impl Handler for TailFetchHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        if request.message.len() > MAX_FETCH_BYTES {
            return Err(RemoteException::status(
                "tail fetch too large",
                "BAD_REQUEST",
            ));
        }
        let fetch = FetchRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed tail fetch", "BAD_REQUEST"))?;
        let answer = self.serve.answer(&fetch);
        answer
            .write(out)
            .map_err(|_| RemoteException::status("tail fetch answer", "INTERNAL_SERVER_ERROR"))
    }
}
