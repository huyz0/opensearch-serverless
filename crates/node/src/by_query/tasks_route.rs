//! `GET /_tasks`, `GET /_tasks/{id}` and `POST /_tasks/{id}/_cancel`.

use std::sync::Arc;

use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{ByQueryService, now_millis};

fn id_of(service: &ByQueryService, req: &Request<'_>) -> Result<u64, RestError> {
    let text = req
        .path_param("task_id")
        .ok_or_else(|| RestError::illegal_argument("a task id is required"))?;
    service.tasks.parse(&text)
}

/// The task routes added to `router`.
///
/// # Errors
///
/// A route registered twice.
pub(super) fn routes(router: Router, service: Arc<ByQueryService>) -> Result<Router, RouteError> {
    let listing = Arc::clone(&service);
    let cancels = Arc::clone(&service);
    router
        .route(
            Method::Get,
            "/_tasks",
            &["actions"],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let actions = req.param("actions");
                let body = listing.tasks.running_json(actions.as_deref(), now_millis());
                resp.send(200, JSON, body.as_bytes())
            },
        )?
        .route(
            Method::Get,
            "/_tasks/{task_id}",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let id = id_of(&service, req)?;
                let body = service.tasks.get(id, now_millis())?;
                resp.send(200, JSON, body.as_bytes())
            },
        )?
        .route(
            Method::Post,
            "/_tasks/{task_id}/_cancel",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                let id = id_of(&cancels, req)?;
                let body = cancels.tasks.cancel(id, now_millis())?;
                resp.send(200, JSON, body.as_bytes())
            },
        )
}
