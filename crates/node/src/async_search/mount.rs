//! Mounting the asynchronous search routes on a node's router.

use std::sync::Arc;

use oss_mem::{MemoryBudget, Pool};
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
) -> Result<(Router, Option<Arc<Service>>), AssembleError> {
    if !config.async_search {
        return Ok((router, None));
    }
    // Results are query output, and the plugin pool is empty on a node that
    // runs no extension, so they are reserved from the query pool.
    let limits = Limits {
        pool: Pool::QueryExecution,
        ..Limits::default()
    };
    let jobs = Jobs::new(&config.node_id, budget, limits, Arc::new(now_millis));
    let service = Arc::new(Service::new(
        jobs,
        Arc::new(SearchRunner::new(search)),
        "opensearch",
        &config.node_id,
    ));
    let router = super::routes(router, &service).map_err(AssembleError::Route)?;
    Ok((router, Some(service)))
}
