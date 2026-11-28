//! The route table: each path by GET and POST, under both prefixes.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::SqlService;

/// The query parameters a SQL request takes.
const PARAMS: &[&str] = &["format", "sanitize", "pretty", "request_timeout"];

type Handler = fn(&SqlService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// Mounts `/_plugins/_sql`, `/_plugins/_ppl`, their `_opendistro` twins and `_explain`.
///
/// # Errors
///
/// A route that cannot be registered.
pub fn routes(router: Router, service: &Arc<SqlService>) -> Result<Router, RouteError> {
    let table: [(&str, Handler); 8] = [
        ("/_plugins/_ppl", SqlService::run_ppl),
        ("/_opendistro/_ppl", SqlService::run_ppl),
        ("/_plugins/_ppl/_explain", SqlService::explain_ppl),
        ("/_opendistro/_ppl/_explain", SqlService::explain_ppl),
        ("/_plugins/_sql", SqlService::run),
        ("/_opendistro/_sql", SqlService::run),
        ("/_plugins/_sql/_explain", SqlService::explain),
        ("/_opendistro/_sql/_explain", SqlService::explain),
    ];
    let mut router = router;
    for (path, handler) in table {
        for method in [Method::Get, Method::Post] {
            let service = Arc::clone(service);
            router = router.route(
                method,
                path,
                PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| handler(&service, req, resp),
            )?;
        }
    }
    Ok(router)
}
