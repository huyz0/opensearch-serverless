//! What a node does once it has enrolled: read the fleet, then tell it.

use std::sync::Arc;

use oss_cluster::lease::Membership;
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;

use super::support::Held;
use crate::handlers::{Duplicate, Registry};
use crate::lease_announce::{ANNOUNCE_GAP, AnnounceHandler, Announcer, Fleet};

/// Has the node answer a peer's announce.
///
/// # Errors
///
/// [`Duplicate`] when the action has a handler.
pub(super) fn register(
    registry: &mut Registry,
    membership: &Arc<Membership>,
) -> Result<(), Duplicate> {
    registry.add(
        AnnounceHandler::ACTION,
        AnnounceHandler::new(Arc::clone(membership) as Arc<dyn Fleet>, ANNOUNCE_GAP),
    )
}

/// Reads the fleet's leases once, so the node knows its peers before the
/// first period passes, then tells them it is here on a thread of its own: a
/// slow peer never delays start-up, and one that misses the announce reads at
/// its next period.
pub(super) fn announce(
    membership: &Arc<Membership>,
    client: Arc<Client>,
    auth: &Arc<Authenticator>,
    node_id: &str,
    ephemeral_id: &str,
) -> std::io::Result<()> {
    let _ = membership.refresh();
    let announcer = Announcer::new(
        client,
        Arc::clone(auth),
        Held(Arc::clone(membership)),
        node_id,
        ephemeral_id,
    );
    std::thread::Builder::new()
        .name("lease-announce".to_owned())
        .spawn(move || {
            announcer.announce();
        })
        .map(drop)
}
