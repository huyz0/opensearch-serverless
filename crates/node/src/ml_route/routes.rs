//! The route table of the ML Commons registry, under `/_plugins/_ml`.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::MlService;

/// The query parameters a registry request accepts: the timeouts every
/// admin route takes.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];

type Handler = fn(&MlService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The model group and connector routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<MlService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 28] = [
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
        (
            Method::Post,
            "/_plugins/_ml/connectors/_create",
            MlService::create_connector,
        ),
        (
            Method::Get,
            "/_plugins/_ml/connectors/_search",
            MlService::search_connectors,
        ),
        (
            Method::Post,
            "/_plugins/_ml/connectors/_search",
            MlService::search_connectors,
        ),
        (
            Method::Put,
            "/_plugins/_ml/connectors/{connector_id}",
            MlService::update_connector,
        ),
        (
            Method::Get,
            "/_plugins/_ml/connectors/{connector_id}",
            MlService::get_connector,
        ),
        (
            Method::Delete,
            "/_plugins/_ml/connectors/{connector_id}",
            MlService::delete_connector,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/meta",
            MlService::register_meta,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/{model_id}/upload_chunk/{chunk_number}",
            MlService::upload_chunk,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/_register",
            MlService::register_model,
        ),
        (
            Method::Get,
            "/_plugins/_ml/models/_search",
            MlService::search_models,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/_search",
            MlService::search_models,
        ),
        (
            Method::Put,
            "/_plugins/_ml/models/{model_id}",
            MlService::update_model,
        ),
        (
            Method::Get,
            "/_plugins/_ml/models/{model_id}",
            MlService::get_model,
        ),
        (
            Method::Delete,
            "/_plugins/_ml/models/{model_id}",
            MlService::delete_model,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/{model_id}/_deploy",
            MlService::deploy_model,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/{model_id}/_undeploy",
            MlService::undeploy_model,
        ),
        (
            Method::Get,
            "/_plugins/_ml/tasks/_search",
            MlService::search_tasks,
        ),
        (
            Method::Post,
            "/_plugins/_ml/tasks/_search",
            MlService::search_tasks,
        ),
        (
            Method::Get,
            "/_plugins/_ml/tasks/{task_id}",
            MlService::get_task,
        ),
        (
            Method::Delete,
            "/_plugins/_ml/tasks/{task_id}",
            MlService::delete_task,
        ),
        (
            Method::Post,
            "/_plugins/_ml/models/{model_id}/_predict",
            MlService::predict_model,
        ),
        (
            Method::Post,
            "/_plugins/_ml/_predict/{algorithm}/{model_id}",
            MlService::predict_algorithm,
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
    budget: &oss_mem::MemoryBudget,
) -> Result<Router, crate::assemble::AssembleError> {
    if !config.ml {
        return Ok(router);
    }
    // Operator setting, never a cluster setting: the endpoints connectors may call.
    let pattern = std::env::var("OSS_ML_TRUSTED_ENDPOINTS").unwrap_or_default();
    let trusted = oss_ml::connector::Trusted::parse(&pattern)
        .map_err(crate::assemble::AssembleError::Security)?;
    let ml = MlService::new(store, &config.base, config.pit_buffer_bytes)
        .with_trusted(trusted)
        .with_node_id(&config.node_id)
        .with_local(super::Local::new(
            budget,
            std::env::var_os("OSS_ML_ONNX_RUNTIME_PATH").map(std::path::PathBuf::from),
        ));
    routes(router, &Arc::new(ml)).map_err(crate::assemble::AssembleError::Route)
}
