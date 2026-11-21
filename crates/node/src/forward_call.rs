//! The signed call both forwarders make: find the owner's lease, encode the
//! request, sign it for that owner, send it, and hand back the answer.
//!
//! A bulk and a get differ in their action, their request and answer types,
//! their timeout and what a refused connect means; everything else is here
//! once. The lease comes from the leases this node already holds, so a call
//! makes no request to the store.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use oss_format::NodeLease;
use oss_transport::WireError;
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Answer, Call, Client, PeerId};
use oss_transport::stream::Output;

use crate::forward::{ForwardError, MAX_FORWARD_BYTES};
use crate::push_link::Leases;

/// The unrevoked lease of node `node_id` and the address it listens on.
///
/// # Errors
///
/// [`ForwardError::NoPeer`]: no unrevoked lease names the node, or its
/// address does not parse.
pub(crate) fn owner_of<L: Leases>(
    leases: &L,
    node_id: &str,
) -> Result<(NodeLease, SocketAddr), ForwardError> {
    let held = leases.current();
    let lease = held
        .iter()
        .find(|l| l.node_id == node_id && !l.revoked)
        .ok_or(ForwardError::NoPeer)?;
    let address: SocketAddr = lease.address.parse().map_err(|_| ForwardError::NoPeer)?;
    Ok((lease.clone(), address))
}

/// Signs `write`'s message for `lease`'s node and sends it to `address`,
/// waiting `timeout` for the answer.
///
/// # Errors
///
/// [`ForwardError::TooLarge`] past [`MAX_FORWARD_BYTES`];
/// [`ForwardError::NoPeer`] when the request cannot be signed; otherwise
/// [`ForwardError::Failed`] with the transport's reason, so a caller can act
/// on a refused connect.
pub(crate) fn signed_call(
    client: &Client,
    auth: &Authenticator,
    action: &'static str,
    (lease, address): (&NodeLease, SocketAddr),
    timeout: Duration,
    write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>,
) -> Result<Answer, ForwardError> {
    // ALLOC: one request message, at most MAX_FORWARD_BYTES.
    let mut message = Vec::new();
    write(&mut Output::new(&mut message, MAX_FORWARD_BYTES)).map_err(|_| ForwardError::TooLarge)?;
    let mac = auth
        .sign(
            action,
            &message,
            &identity(&lease.node_id, &lease.ephemeral_id),
            &[],
        )
        .ok_or(ForwardError::NoPeer)?;
    // ALLOC: the MAC header, one entry.
    let headers = [(MAC_HEADER.to_owned(), mac)];
    let peer = PeerId {
        address,
        // ALLOC: one ephemeral id a call.
        ephemeral_id: Some(lease.ephemeral_id.clone()),
    };
    let call = Call {
        action,
        headers: &headers,
        compress: false,
        deadline: Instant::now()
            .checked_add(timeout)
            .ok_or(ForwardError::NoPeer)?,
    };
    client
        .call(&peer, call, |out| out.bytes(&message))
        .map_err(ForwardError::Failed)
}
