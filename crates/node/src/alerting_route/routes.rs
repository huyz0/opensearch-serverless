//! The route table of the monitors, on `/_plugins/_alerting` and the legacy
//! `/_opendistro/_alerting`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::AlertingService;
use crate::assemble::AssembleError;

/// The query parameters a monitor request accepts: the plugin's own, and
/// the timeouts every admin route takes.
const PARAMS: &[&str] = &[
    "if_seq_no",
    "if_primary_term",
    "refresh",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

type Handler = fn(&AlertingService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

fn table() -> [(Method, &'static str, Handler); 4] {
    [
        (Method::Post, "/monitors", AlertingService::create),
        (Method::Put, "/monitors/{id}", AlertingService::update),
        (Method::Get, "/monitors/{id}", AlertingService::get),
        (Method::Delete, "/monitors/{id}", AlertingService::delete),
    ]
}

/// The monitor routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<AlertingService>) -> Result<Router, RouteError> {
    let mut router = router;
    for root in ["/_plugins/_alerting", "/_opendistro/_alerting"] {
        for (method, tail, handler) in table() {
            // ALLOC: the router keeps its patterns for the life of the process.
            let path: &'static str = Box::leak(format!("{root}{tail}").into_boxed_str());
            let service = Arc::clone(service);
            router = router.route(
                method,
                path,
                PARAMS,
                move |q: &mut Request<'_>, r: &mut Response<'_>| handler(&service, q, r),
            )?;
        }
    }
    Ok(router)
}

/// The monitor routes over `store`, with the default limits.
///
/// # Errors
///
/// [`AssembleError::Route`] for a route registered twice.
pub fn install(
    router: Router,
    store: &Arc<dyn oss_blob::BlobStore>,
    base: &str,
) -> Result<Router, AssembleError> {
    routes(router, &Arc::new(AlertingService::new(store, base))).map_err(AssembleError::Route)
}
