//! The relayed asynchronous search call's handler slot, and what fills it
//! once the node is assembled.

use std::sync::{Arc, OnceLock};

use oss_cluster::lease::Membership;
use oss_transport::actions::async_search::ASYNC_SEARCH_ACTION;
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::server::Handler;

use super::BootError;
use crate::boot::transport::Late;

/// The handler slot `ASYNC_SEARCH_ACTION` is registered with, empty until
/// [`wire`].
pub(super) fn register(
    registry: &mut crate::handlers::Registry,
) -> Result<Arc<OnceLock<Arc<dyn Handler>>>, BootError> {
    let late = Late::default();
    let slot = late.slot();
    registry
        .add(ASYNC_SEARCH_ACTION, late)
        .map_err(|e| BootError::Cluster(e.to_string()))?;
    Ok(slot)
}

/// Answers relayed calls from this node's own searches, and sends an id of
/// another node to that node. A node with the setting off leaves the slot
/// empty.
pub(super) fn wire(
    assembled: &crate::assemble::Assembled,
    slot: &OnceLock<Arc<dyn Handler>>,
    client: &Arc<Client>,
    auth: &Arc<Authenticator>,
    membership: &Arc<Membership>,
) {
    use super::support::Held;
    let Some(service) = &assembled.async_search else {
        return;
    };
    slot.set(Arc::new(crate::async_search::AsyncSearchHandler::new(
        Arc::clone(service),
    )))
    .ok();
    service.set_relay(Arc::new(crate::async_search::AsyncSearchForwarder::new(
        Arc::clone(client),
        Arc::clone(auth),
        Held(Arc::clone(membership)),
    )));
}
