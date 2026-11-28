//! The route table of the ML Commons registry, under `/_plugins/_ml`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::MlService;

/// The query parameters a registry request accepts: the timeouts every
/// admin route takes.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];

type Handler = fn(&MlService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The model group routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<MlService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 6] = [
        (
            Method::Post,
            "/_plugins/_ml/model_groups/_register",
            MlService::register_group,
        ),
        (
            Method::Get,
            "/_plugins/_ml/model_groups/_search",
            MlService::search_groups,
        ),
        (
            Method::Post,
            "/_plugins/_ml/model_groups/_search",
            MlService::search_groups,
        ),
        (
            Method::Put,
            "/_plugins/_ml/model_groups/{model_group_id}",
            MlService::update_group,
        ),
        (
            Method::Get,
            "/_plugins/_ml/model_groups/{model_group_id}",
            MlService::get_group,
        ),
        (
            Method::Delete,
            "/_plugins/_ml/model_groups/{model_group_id}",
            MlService::delete_group,
        ),
    ];
    let mut router = router;
    for (method, path, handler) in table {
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

/// Mounts the registry routes when the node's ML switch is on.
pub(crate) fn mount(
    router: Router,
    store: &Arc<dyn oss_blob::BlobStore>,
    config: &crate::assemble::Config,
) -> Result<Router, crate::assemble::AssembleError> {
    if !config.ml {
        return Ok(router);
    }
    let ml = MlService::new(store, &config.base, config.pit_buffer_bytes);
    routes(router, &Arc::new(ml)).map_err(crate::assemble::AssembleError::Route)
}
