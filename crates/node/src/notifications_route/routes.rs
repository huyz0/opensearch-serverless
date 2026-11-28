//! The route table of the notifications plugin, on `/_plugins/_notifications`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::NotificationsService;

/// The query parameters a request accepts: the plugin's own, and the
/// timeouts every admin route takes.
const PARAMS: &[&str] = &[
    "config_id",
    "config_id_list",
    "from_index",
    "max_items",
    "sort_field",
    "sort_order",
    "config_type",
    "is_enabled",
    "name",
    "description",
    "text_query",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

type Handler =
    fn(&NotificationsService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The config routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<NotificationsService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 9] = [
        (Method::Post, "/configs", NotificationsService::create),
        (Method::Get, "/configs", NotificationsService::list),
        (
            Method::Delete,
            "/configs",
            NotificationsService::delete_many,
        ),
        (Method::Get, "/configs/{id}", NotificationsService::get_one),
        (Method::Put, "/configs/{id}", NotificationsService::replace),
        (
            Method::Delete,
            "/configs/{id}",
            NotificationsService::delete_one,
        ),
        (Method::Get, "/features", NotificationsService::features),
        (Method::Get, "/channels", NotificationsService::channels),
        (
            Method::Post,
            "/feature/test/{id}",
            NotificationsService::test_send,
        ),
    ];
    let mut router = router;
    for (method, tail, handler) in table {
        // ALLOC: the router keeps its patterns for the life of the process.
        let path: &'static str =
            Box::leak(format!("/_plugins/_notifications{tail}").into_boxed_str());
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
