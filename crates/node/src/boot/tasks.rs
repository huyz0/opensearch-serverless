//! The forwarded task call's handler slot, and what fills it once the node is
//! assembled.

use std::sync::{Arc, OnceLock};

use oss_transport::actions::tasks::TASKS_ACTION;
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::server::Handler;

use super::BootError;
use crate::boot::transport::Late;
use oss_cluster::lease::Membership;

/// The handler slot `TASKS_ACTION` is registered with, empty until
/// [`wire`].
pub(super) fn register(
    registry: &mut crate::handlers::Registry,
) -> Result<Arc<OnceLock<Arc<dyn Handler>>>, BootError> {
    let late = Late::default();
    let slot = late.slot();
    registry
        .add(TASKS_ACTION, late)
        .map_err(|e| BootError::Cluster(e.to_string()))?;
    Ok(slot)
}

/// Answers forwarded task calls from this node's tasks, and sends a task id
/// of another node to that node.
#[cfg(feature = "coordinator")]
pub(super) fn wire(
    assembled: &crate::assemble::Assembled,
    slot: &OnceLock<Arc<dyn Handler>>,
    client: &Arc<Client>,
    auth: &Arc<Authenticator>,
    membership: &Arc<Membership>,
) {
    use super::support::Held;
    slot.set(Arc::new(crate::by_query::TaskHandler::new(Arc::clone(
        &assembled.tasks,
    ))))
    .ok();
    assembled
        .tasks
        .set_forward(Arc::new(crate::by_query::TaskForwarder::new(
            Arc::clone(client),
            Arc::clone(auth),
            Held(Arc::clone(membership)),
        )));
}

/// Without the coordinator role a node keeps no tasks, so the slot stays empty.
#[cfg(not(feature = "coordinator"))]
pub(super) fn wire(
    _: &crate::assemble::Assembled,
    _: &OnceLock<Arc<dyn Handler>>,
    _: &Arc<Client>,
    _: &Arc<Authenticator>,
    _: &Arc<Membership>,
) {
}
