//! The route table of the policies, on `/_plugins/_ism` and the legacy
//! `/_opendistro/_ism`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::IsmService;

/// The query parameters a policy request accepts: the plugin's own, and the
/// timeouts every admin route takes.
const PARAMS: &[&str] = &[
    "if_seq_no",
    "if_primary_term",
    "from",
    "size",
    "queryString",
    "sortField",
    "sortOrder",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

type Handler = fn(&IsmService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The policy routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<IsmService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 4] = [
        (Method::Put, "/policies/{id}", IsmService::put),
        (Method::Get, "/policies/{id}", IsmService::get),
        (Method::Delete, "/policies/{id}", IsmService::delete),
        (Method::Get, "/policies", IsmService::list),
    ];
    let mut router = router;
    for root in ["/_plugins/_ism", "/_opendistro/_ism"] {
        for (method, tail, handler) in table {
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
