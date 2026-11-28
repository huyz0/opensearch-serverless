//! The route table of the monitors, on `/_plugins/_alerting` and the legacy
//! `/_opendistro/_alerting`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::AlertingService;
use crate::assemble::{AssembleError, Config};
use oss_meta::DescriptorStore;

/// The query parameters a monitor request accepts: the plugin's own, and
/// the timeouts every admin route takes.
const PARAMS: &[&str] = &[
    "if_seq_no",
    "if_primary_term",
    "q",
    "refresh",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

/// The parameters of the findings search.
const FINDINGS: &[&str] = &[
    "findingId",
    "sortString",
    "sortOrder",
    "size",
    "startIndex",
    "searchString",
];

type Handler = fn(&AlertingService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

type Entry = (Method, &'static str, &'static [&'static str], Handler);

fn table() -> [Entry; 17] {
    use AlertingService as S;
    [
        (Method::Post, "/monitors", PARAMS, S::create),
        (Method::Post, "/monitors/_search", PARAMS, S::search),
        (Method::Get, "/monitors/_search", PARAMS, S::search),
        (Method::Put, "/monitors/{id}", PARAMS, S::update),
        (Method::Get, "/monitors/{id}", PARAMS, S::get),
        (Method::Head, "/monitors/{id}", PARAMS, S::head),
        (Method::Delete, "/monitors/{id}", PARAMS, S::delete),
        (Method::Get, "/destinations", PARAMS, S::destinations),
        (Method::Get, "/destinations/{id}", PARAMS, S::destinations),
        (
            Method::Post,
            "/destinations/email_accounts/_search",
            PARAMS,
            S::email_search,
        ),
        (
            Method::Get,
            "/destinations/email_accounts/_search",
            PARAMS,
            S::email_search,
        ),
        (
            Method::Post,
            "/destinations/email_groups/_search",
            PARAMS,
            S::email_search,
        ),
        (
            Method::Get,
            "/destinations/email_groups/_search",
            PARAMS,
            S::email_search,
        ),
        (Method::Get, "/findings/_search", FINDINGS, S::findings),
        (Method::Get, "/stats", PARAMS, S::stats),
        (Method::Get, "/stats/{metric}", PARAMS, S::stats),
        (Method::Get, "/{node}/stats", PARAMS, S::stats),
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
        for (method, tail, params, handler) in table() {
            // ALLOC: the router keeps its patterns for the life of the process.
            let path: &'static str = Box::leak(format!("{root}{tail}").into_boxed_str());
            let service = Arc::clone(service);
            router = router.route(
                method,
                path,
                params,
                move |q: &mut Request<'_>, r: &mut Response<'_>| handler(&service, q, r),
            )?;
        }
    }
    Ok(router)
}

/// The monitor routes over `store`, with the default limits, checking the
/// indices a monitor names against the node's index records.
///
/// # Errors
///
/// [`AssembleError::Route`] for a route registered twice;
/// [`AssembleError::Store`] for a base the records refuse.
pub fn install(
    router: Router,
    store: &Arc<dyn oss_blob::BlobStore>,
    config: &Config,
) -> Result<Router, AssembleError> {
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    let service = AlertingService::new(store, &config.base)
        .with_descriptors(descriptors, config.pit_buffer_bytes);
    routes(router, &Arc::new(service)).map_err(AssembleError::Route)
}
