//! Mounting the asynchronous search routes on a node's router.

use std::sync::Arc;

use oss_mem::MemoryBudget;
use oss_rest::Router;

use super::{SearchRunner, Service};
use crate::assemble::{AssembleError, Config};
use crate::clock::now_millis;
use crate::jobs::{Jobs, Limits};
use crate::sql_route::SqlBackend;

/// Registers the routes over the node's own search when the setting is on;
/// with it off those paths answer as an unknown route.
pub(crate) fn mount(
    router: Router,
    config: &Config,
    budget: &MemoryBudget,
    search: Arc<dyn SqlBackend>,
) -> Result<Router, AssembleError> {
    if !config.async_search {
        return Ok(router);
    }
    let jobs = Jobs::new(
        &config.node_id,
        budget,
        Limits::default(),
        Arc::new(now_millis),
    );
    let service = Service::new(
        jobs,
        Arc::new(SearchRunner::new(search)),
        "opensearch",
        &config.node_id,
    );
    super::routes(router, &Arc::new(service)).map_err(AssembleError::Route)
}
