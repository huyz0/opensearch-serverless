//! The notifications routes and the in-process sender a node assembles when
//! `serverless.notifications.enabled` is on (it is by default).

use std::sync::Arc;
use std::time::Duration;

use oss_blob::BlobStore;
use oss_mem::MemoryBudget;
use oss_rest::Router;

use crate::assemble::{AssembleError, Config};
use crate::notifications_route::{
    Deliverer, EgressPolicy, Limits, NotificationsService, Notifier, routes, system_lookup,
};

/// How long a node keeps a channel config before it checks the set again.
const CACHE_TTL: Duration = Duration::from_secs(30);

/// Registers the notifications routes on `router` and builds the sender the
/// alerting and ISM plugins call. Both use one deliverer, so one egress
/// policy and one bound on concurrent sends. None when notifications are off.
pub(crate) fn mount(
    router: Router,
    config: &Config,
    store: &Arc<dyn BlobStore>,
    budget: &MemoryBudget,
) -> Result<(Router, Option<Arc<Notifier>>), AssembleError> {
    if !config.notifications {
        return Ok((router, None));
    }
    let deliverer = Arc::new(Deliverer::new(
        system_lookup(),
        EgressPolicy::from_env(),
        Limits::default(),
    ));
    let service = NotificationsService::new(store, &config.base, config.pit_buffer_bytes)
        .with_deliverer(Arc::clone(&deliverer));
    let router = routes(router, &Arc::new(service)).map_err(AssembleError::Route)?;
    let notifier = Arc::new(Notifier::new(
        store,
        &config.base,
        budget.clone(),
        deliverer,
        CACHE_TTL,
    ));
    Ok((router, Some(notifier)))
}
