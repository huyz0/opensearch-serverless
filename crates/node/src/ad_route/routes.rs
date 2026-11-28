//! The route table of the detectors, on `/_plugins/_anomaly_detection` and
//! the legacy `/_opendistro/_anomaly_detection`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::AdService;

/// The query parameters a detector request accepts: the plugin's own, and the
/// timeouts every admin route takes.
const PARAMS: &[&str] = &[
    "if_seq_no",
    "if_primary_term",
    "job",
    "task",
    "from",
    "size",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

/// What `_topAnomalies` takes besides the detector's own: whether to read a historical analysis.
const TOP_PARAMS: &[&str] = &[
    "historical",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

type Handler = fn(&AdService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The detector routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<AdService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 20] = [
        (Method::Post, "/detectors", AdService::create),
        (Method::Put, "/detectors/{id}", AdService::replace),
        (Method::Get, "/detectors/{id}", AdService::get),
        (Method::Delete, "/detectors/{id}", AdService::delete),
        (Method::Get, "/detectors/_search", AdService::search),
        (Method::Post, "/detectors/_search", AdService::search),
        (Method::Get, "/detectors/_count", AdService::count),
        (Method::Post, "/detectors/_count", AdService::count),
        (Method::Post, "/detectors/_validate", AdService::validate),
        (
            Method::Post,
            "/detectors/_validate/{type}",
            AdService::validate,
        ),
        (Method::Post, "/detectors/_preview", AdService::preview),
        (Method::Post, "/detectors/{id}/_preview", AdService::preview),
        (Method::Post, "/detectors/{id}/_start", AdService::start),
        (Method::Post, "/detectors/{id}/_stop", AdService::stop),
        (Method::Get, "/detectors/{id}/_profile", AdService::profile),
        (
            Method::Get,
            "/detectors/{id}/_profile/{type}",
            AdService::profile,
        ),
        (
            Method::Get,
            "/detectors/results/_search",
            AdService::results,
        ),
        (
            Method::Post,
            "/detectors/results/_search",
            AdService::results,
        ),
        (
            Method::Get,
            "/detectors/results/_search/{result_index}",
            AdService::results,
        ),
        (
            Method::Post,
            "/detectors/results/_search/{result_index}",
            AdService::results,
        ),
    ];
    let mut router = router;
    for root in [
        "/_plugins/_anomaly_detection",
        "/_opendistro/_anomaly_detection",
    ] {
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
    for root in [
        "/_plugins/_anomaly_detection",
        "/_opendistro/_anomaly_detection",
    ] {
        for method in [Method::Get, Method::Post] {
            // ALLOC: the router keeps its patterns for the life of the process.
            let path: &'static str = Box::leak(
                format!("{root}/detectors/{{id}}/results/_topAnomalies").into_boxed_str(),
            );
            let service = Arc::clone(service);
            router = router.route(
                method,
                path,
                TOP_PARAMS,
                move |q: &mut Request<'_>, r: &mut Response<'_>| {
                    AdService::top_anomalies(&service, q, r)
                },
            )?;
        }
    }
    Ok(router)
}
