//! Mounting the SQL routes on a node's router.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_rest::Router;

use super::{AllowAll, NameGuard, SqlBackend, SqlGuard, SqlLimits, SqlService};
use crate::assemble::{AssembleError, Config};
use crate::search::SearchService;
use crate::security_state::SecurityState;

/// Registers the SQL routes over `search`: always, so a node with the setting
/// off answers the plugin's disabled error rather than an unknown route.
pub(crate) fn mount<S: BlobStore + 'static>(
    router: Router,
    config: &Config,
    search: &Arc<SearchService<S>>,
    security: Option<&Arc<SecurityState>>,
) -> Result<Router, AssembleError> {
    let guard: Arc<dyn SqlGuard> = match security {
        Some(state) => Arc::new(NameGuard::new(Arc::clone(state))),
        None => Arc::new(AllowAll),
    };
    let limits = SqlLimits {
        enabled: config.sql,
        size_limit: config.sql_size_limit,
        max_request_bytes: config.sql_max_request_bytes,
        max_buckets: config.sql_max_buckets,
    };
    let backend: Arc<dyn SqlBackend> = Arc::clone(search) as _;
    let service = Arc::new(SqlService::new(backend, guard, limits));
    super::routes(router, &service).map_err(AssembleError::Route)
}
