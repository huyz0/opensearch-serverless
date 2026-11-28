//! The route table of the observability objects.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::ObservabilityService;

/// The query parameters an object request accepts: the timeouts every admin
/// route takes.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];

type Handler =
    fn(&ObservabilityService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The object routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<ObservabilityService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, &'static str, Handler); 2] = [
        (
            Method::Post,
            "/_plugins/_observability/object",
            "create",
            ObservabilityService::create,
        ),
        (
            Method::Get,
            "/_plugins/_observability/object/{id}",
            "read",
            ObservabilityService::get,
        ),
    ];
    let mut router = router;
    for (method, path, _name, handler) in table {
        let service = Arc::clone(service);
        router = router.route(
            method,
            path,
            PARAMS,
            move |q: &mut Request<'_>, r: &mut Response<'_>| handler(&service, q, r),
        )?;
    }
    Ok(router)
}
