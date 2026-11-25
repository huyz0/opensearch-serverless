//! A document get forwarded to the node that owns its shard: the owner's
//! handler, and the send side.
//!
//! Only the owner holds writes that are acknowledged and not yet published,
//! so a realtime get of a shard held elsewhere goes to its owner, as the
//! shell's forwarded get does. The owner is found in the leases this node
//! already holds, so a forward makes no request to the store.

use std::sync::Arc;
use std::time::Duration;

use oss_transport::actions::read::{GET_ACTION, GetRequest, GetResponse};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::forward::ForwardError;
use crate::forward_call::{owner_of, signed_call};
use crate::push_link::Leases;
use crate::writer::WriterRole;

/// How long a forwarded get waits for its owner.
pub const GET_TIMEOUT: Duration = Duration::from_secs(10);

/// Answers forwarded gets from the shards this node holds.
pub struct GetHandler {
    role: Arc<WriterRole>,
    node_id: String,
}

impl GetHandler {
    /// A handler reading from `role`, answering as node `node_id`.
    #[must_use]
    pub fn new(role: Arc<WriterRole>, node_id: &str) -> Self {
        Self {
            role,
            // ALLOC: one node id, at start-up.
            node_id: node_id.to_owned(),
        }
    }
}

impl Handler for GetHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let get = GetRequest::read(&mut Input::new(request.message))
            // ALLOC: error path; one message.
            .map_err(|e| {
                RemoteException::status(&format!("bad get request: {e:?}"), "BAD_REQUEST")
            })?;
        let shard = u32::try_from(get.shard)
            .map_err(|_| RemoteException::status("negative shard", "BAD_REQUEST"))?;
        let index = self
            .role
            .index_info(&get.index)
            .filter(|i| get.index_uuid.as_ref().is_none_or(|u| *u == i.uuid))
            .ok_or_else(|| {
                // ALLOC: error path; one message.
                RemoteException::status(&format!("no such index [{}]", get.index), "NOT_FOUND")
            })?;
        let found = self
            .role
            .get(&index, shard, &get.id, get.realtime)
            // ALLOC: error path; one status name.
            .map_err(|e| RemoteException::status(&e, "SERVICE_UNAVAILABLE"))?;
        let answer = match found {
            Some(doc) => GetResponse {
                // ALLOC: the answer's strings, one document.
                owner_node_id: self.node_id.clone(),
                id: get.id,
                found: true,
                source: Some(String::from_utf8(doc.source).map_err(|_| {
                    RemoteException::status(
                        "the stored source is not UTF-8",
                        "INTERNAL_SERVER_ERROR",
                    )
                })?),
                seq_no: doc.seq_no,
                primary_term: doc.primary_term,
                version: doc.version,
                routing: doc.routing,
            },
            None => GetResponse {
                // ALLOC: as above.
                owner_node_id: self.node_id.clone(),
                id: get.id,
                ..GetResponse::default()
            },
        };
        answer.write(out).map_err(|_| {
            RemoteException::status("the answer is too large to send", "INTERNAL_SERVER_ERROR")
        })
    }
}

/// The prefix of the build version a node of this project publishes in its
/// lease. Only such a peer reads the trailing `realtime` byte of a forwarded
/// get; a Java peer refuses a message it did not read whole, so it is sent
/// the Java layout, which reads realtime.
pub const BUILD_PREFIX: &str = "oss-rust/";

fn peer_reads_realtime(lease: &oss_format::NodeLease) -> bool {
    same_build(lease)
}

/// Whether a member's lease names a node of this build family (the
/// [`BUILD_PREFIX`] version): one that reads the tail overlay and runs the
/// proactive takeover. A lease with no version is not.
pub(crate) fn same_build(lease: &oss_format::NodeLease) -> bool {
    lease
        .version
        .as_deref()
        .is_some_and(|v| v.starts_with(BUILD_PREFIX))
}

/// The call a forwarded get makes to an owner.
pub trait GetSend: Send + Sync {
    /// Sends `request` to node `node_id` and reads its answer.
    ///
    /// # Errors
    ///
    /// [`ForwardError`]: nothing sent, or the call failed.
    fn get(&self, node_id: &str, request: &GetRequest) -> Result<GetResponse, ForwardError>;
}

impl<L: Leases> GetSend for GetForwarder<L> {
    fn get(&self, node_id: &str, request: &GetRequest) -> Result<GetResponse, ForwardError> {
        GetForwarder::get(self, node_id, request)
    }
}

/// Sends gets to shard owners over the transport client, signed.
pub struct GetForwarder<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
    timeout: Duration,
}

impl<L: Leases> GetForwarder<L> {
    /// A forwarder calling through `client`, signing with `auth`, finding
    /// owners in `leases`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, leases: L) -> Self {
        Self {
            client,
            auth,
            leases,
            timeout: GET_TIMEOUT,
        }
    }

    /// The same, waiting `timeout` for an owner.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sends `request` to node `node_id` and reads its answer.
    ///
    /// # Errors
    ///
    /// [`ForwardError`]: nothing sent, or the call failed.
    pub fn get(&self, node_id: &str, request: &GetRequest) -> Result<GetResponse, ForwardError> {
        let (lease, address) = owner_of(&self.leases, node_id)?;
        let answer = signed_call(
            &self.client,
            &self.auth,
            GET_ACTION,
            (&lease, address),
            self.timeout,
            64,
            |out| {
                if peer_reads_realtime(&lease) {
                    request.write_extended(out)
                } else {
                    request.write(out)
                }
            },
        )?;
        GetResponse::read(&mut Input::new(answer.message())).map_err(|_| ForwardError::BadAnswer)
    }
}
