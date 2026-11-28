//! `POST /_plugins/_knn/models/{id}/_train` and `/models/_train`: the
//! request's checks against the model store and the training index's
//! mapping (docs/specs/knn-plugin-api, ADR 0086).
//!
//! A request that passes every check is answered 501: the training job is
//! not served yet, so nothing is stored.

use std::sync::Arc;

use oss_doc::mapping::{FieldType, Mapping, ParseOptions, Value as MappingValue};
use oss_meta::DescriptorStore;
use oss_rest::{Method, Request, Response, RestError, RouteError, Router};
use serde_json::Value;

use crate::knn_model_route::KnnModelService;
use crate::knn_train_request::{TrainRequest, parse};
use crate::route_util::{read_body, refuse};

/// The largest training body accepted: a method with its parameters.
const MAX_BODY: usize = 64 * 1024;
const PARAMS: &[&str] = &["master_timeout", "timeout", "filter_path"];

fn invalid(reason: String) -> RestError {
    refuse(
        400,
        "action_request_validation_exception",
        format!("Validation Failed: 1: {reason};"),
    )
}

/// Serves the training routes over the model store and the index descriptors.
pub struct KnnTrainService {
    models: Arc<KnnModelService>,
    descriptors: DescriptorStore,
    buffer_bytes: usize,
}

impl KnnTrainService {
    /// A service answering for `models`, reading mappings through `descriptors`.
    pub fn new(
        models: Arc<KnnModelService>,
        descriptors: DescriptorStore,
        buffer_bytes: usize,
    ) -> Self {
        Self {
            models,
            descriptors,
            buffer_bytes,
        }
    }

    /// The dimension of `field` in `index`'s mapping, or the plugin's refusal.
    fn field_dimension(&self, index: &str, field: &str) -> Result<i64, RestError> {
        // ALLOC: one record buffer a request, buffer_bytes.
        let mut buf = vec![0u8; self.buffer_bytes];
        let record = self
            .descriptors
            .get(index, &mut buf)
            .map_err(|e| refuse(500, "knn_model_store_failure", e.to_string()))?
            .ok_or_else(|| invalid(format!("Index \"{index}\" does not exist.")))?;
        let missing = || invalid(format!("Field \"{field}\" does not exist."));
        let text = record.mapping.as_deref().ok_or_else(missing)?;
        let options = ParseOptions {
            knn: Some(true),
            nested: true,
        };
        let mapping = Mapping::parse_with(text.as_bytes(), options).map_err(|_| missing())?;
        let leaf = mapping
            .leaves()
            .into_iter()
            .find(|(path, leaf)| path == field && leaf.ty() == FieldType::Knn)
            .map(|(_, leaf)| leaf)
            .ok_or_else(missing)?;
        match leaf.get("dimension") {
            Some(MappingValue::Int(d)) => Ok(i64::from(*d)),
            _ => Err(missing()),
        }
    }

    fn check(&self, id: Option<&str>, request: &TrainRequest) -> Result<(), RestError> {
        if let Some(id) = id
            && self.models.exists(id)?
        {
            return Err(invalid(format!("Model with id=\"{id}\" already exists")));
        }
        let dimension = self.field_dimension(&request.training_index, &request.training_field)?;
        if dimension != i64::from(request.dimension) {
            return Err(invalid(format!(
                "Field \"{}\" has dimension {dimension}, which is different from dimension specified in the training request: {}",
                request.training_field, request.dimension
            )));
        }
        Ok(())
    }

    fn train(&self, req: &mut Request<'_>, _resp: &mut Response<'_>) -> Result<(), RestError> {
        let id = req.path_param("id").map(std::borrow::Cow::into_owned);
        let raw = read_body(req, MAX_BODY, "the training request is too large")?;
        let body: Value = serde_json::from_slice(&raw).map_err(|e| {
            refuse(
                400,
                "illegal_argument_exception",
                format!("invalid body: {e}"),
            )
        })?;
        let request = parse(&body)?;
        self.check(id.as_deref(), &request)?;
        Err(refuse(
            501,
            "not_implemented",
            "training a model is not served yet",
        ))
    }
}

/// The training routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<KnnTrainService>) -> Result<Router, RouteError> {
    let mut router = router;
    for path in [
        "/_plugins/_knn/models/{id}/_train",
        "/_plugins/_knn/models/_train",
    ] {
        let service = Arc::clone(service);
        router = router.route(
            Method::Post,
            path,
            PARAMS,
            move |q: &mut Request<'_>, r: &mut Response<'_>| service.train(q, r),
        )?;
    }
    Ok(router)
}

/// Mounts the model and training routes when the node's k-NN switch is on.
pub(crate) fn mount(
    router: Router,
    store: &Arc<dyn oss_blob::BlobStore>,
    config: &crate::assemble::Config,
) -> Result<Router, crate::assemble::AssembleError> {
    use crate::assemble::AssembleError;
    if !config.knn {
        return Ok(router);
    }
    let models = Arc::new(KnnModelService::new(
        store,
        &config.base,
        config.pit_buffer_bytes,
    ));
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(|e| AssembleError::Store(e.to_string()))?;
    let train = KnnTrainService::new(Arc::clone(&models), descriptors, config.pit_buffer_bytes);
    let router = crate::knn_model_route::routes(router, &models).map_err(AssembleError::Route)?;
    routes(router, &Arc::new(train)).map_err(AssembleError::Route)
}
