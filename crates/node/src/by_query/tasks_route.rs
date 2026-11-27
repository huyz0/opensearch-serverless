//! `GET /_tasks`, `GET /_tasks/{id}` and `POST /_tasks/{id}/_cancel`.

use std::sync::Arc;

use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use oss_transport::actions::tasks::{TaskRequest, TaskResponse};

use super::tasks::Target;
use super::{ByQueryService, now_millis};

/// Answers a read or a cancel of the task `req` names: here, or by the node
/// the id names.
fn answer(
    service: &ByQueryService,
    req: &Request<'_>,
    resp: &mut Response<'_>,
    cancel: bool,
) -> Result<(), RestError> {
    let text = req
        .path_param("task_id")
        .ok_or_else(|| RestError::illegal_argument("a task id is required"))?;
    let (status, body) = match service.tasks.route(&text)? {
        Target::Local(number) => {
            let request = TaskRequest { cancel, number };
            let TaskResponse { status, body } = service.tasks.call(request, now_millis());
            (status, body)
        }
        Target::Remote { node, number } => {
            let TaskResponse { status, body } = service
                .tasks
                .remote(&node, TaskRequest { cancel, number })?;
            (status, body)
        }
    };
    resp.send(status, JSON, body.as_bytes())
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
                answer(&service, req, resp, false)
            },
        )?
        .route(
            Method::Post,
            "/_tasks/{task_id}/_cancel",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| answer(&cancels, req, resp, true),
        )
}
