//! The route table of the report definitions.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::ReportsService;

/// The query parameters a definition request accepts.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];
/// The query parameters a list accepts.
const LIST_PARAMS: &[&str] = &[
    "fromIndex",
    "maxItems",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

type Handler = fn(&ReportsService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// Mounts the definition routes when the node's reports switch is on.
///
/// # Errors
///
/// A route registered twice.
pub(crate) fn mount(
    router: Router,
    store: &Arc<dyn oss_blob::BlobStore>,
    config: &crate::assemble::Config,
) -> Result<Router, crate::assemble::AssembleError> {
    if !config.reports {
        return Ok(router);
    }
    let service = ReportsService::new(store, &config.base, config.pit_buffer_bytes);
    routes(router, &Arc::new(service)).map_err(crate::assemble::AssembleError::Route)
}

/// The definition routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<ReportsService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, &'static [&'static str], Handler); 6] = [
        (
            Method::Post,
            "/_plugins/_reports/definition",
            PARAMS,
            ReportsService::create,
        ),
        (
            Method::Get,
            "/_plugins/_reports/definitions",
            LIST_PARAMS,
            ReportsService::list,
        ),
        (
            Method::Get,
            "/_plugins/_reports/definition/{id}",
            PARAMS,
            ReportsService::get,
        ),
        (
            Method::Put,
            "/_plugins/_reports/definition/{id}",
            PARAMS,
            ReportsService::replace,
        ),
        (
            Method::Delete,
            "/_plugins/_reports/definition/{id}",
            PARAMS,
            ReportsService::delete,
        ),
        (
            Method::Get,
            "/_plugins/_reports/definition",
            LIST_PARAMS,
            ReportsService::list,
        ),
    ];
    let mut router = router;
    for (method, path, params, handler) in table {
        let service = Arc::clone(service);
        router = router.route(
            method,
            path,
            params,
            move |q: &mut Request<'_>, r: &mut Response<'_>| handler(&service, q, r),
        )?;
    }
    Ok(router)
}
