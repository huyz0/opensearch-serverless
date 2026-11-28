//! `/_plugins/_knn/models`: the k-NN plugin's model store, served natively
//! over a document set (docs/specs/knn-plugin-api, ADR 0086).
//!
//! A model is one document of the `knn/models/` set, kept as the plugin's
//! own `_source` (`model_id`, `model_blob`, `state`, `timestamp`,
//! `description`, `error`, `space_type`, `dimension`, `engine`). A read is
//! the marker's read and the content's; nothing lists: a search reads the
//! marker's names and the page of documents it returns. Shapes follow the
//! plugin's documentation and are provisional until a stock recording
//! exists.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};

/// The plugin's model index, shown as every hit's `_index`.
const INDEX: &str = ".opensearch-knn-models";
/// The most hits a search page returns.
const MAX_PAGE: usize = 100;
/// The query parameters a model request accepts.
const PARAMS: &[&str] = &["from", "size", "master_timeout", "timeout", "filter_path"];

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn not_found(id: &str) -> RestError {
    refuse(
        404,
        "resource_not_found_exception",
        // ALLOC: error path; one message.
        format!("Model \"{id}\" does not exist"),
    )
}

fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "knn_model_store_failure", other.to_string()),
    }
}

/// Serves the model routes over one deployment's object store.
pub struct KnnModelService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
}

impl KnnModelService {
    /// A service over the models under `base` in `store`; a request holds up
    /// to `buffer_bytes` for a marker.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, buffer_bytes: usize) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            buffer_bytes,
        }
    }

    fn set(&self) -> Result<DocSet, RestError> {
        DocSet::new(Arc::clone(&self.store), &self.base, DocKind::KnnModels).map_err(|e| failed(&e))
    }

    fn marker(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let bytes = DocKind::KnnModels.max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    fn id_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a model id is required"))
    }

    fn stored(
        &self,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Value>, RestError> {
        match self.set()?.get(id, marker, doc) {
            Ok(Some(n)) => serde_json::from_slice(doc.get(..n).unwrap_or_default())
                .map(Some)
                .map_err(|_| {
                    refuse(
                        500,
                        "knn_model_store_failure",
                        "a stored model is unreadable",
                    )
                }),
            Ok(None) => Ok(None),
            Err(e) => Err(failed(&e)),
        }
    }

    /// `GET /_plugins/_knn/models/{id}`.
    fn get(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let model = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        resp.send(200, JSON, model.to_string().as_bytes())
    }

    /// `DELETE /_plugins/_knn/models/{id}`: refused while the model trains.
    fn delete(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let model = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        if model["state"] == "training" {
            return Err(bad(format!(
                "Cannot delete model \"{id}\". Model is still in training"
            )));
        }
        if !self
            .set()?
            .delete(&id, &mut marker)
            .map_err(|e| failed(&e))?
        {
            return Err(not_found(&id));
        }
        let body = json!({"model_id": id, "result": "deleted"});
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `GET|POST /_plugins/_knn/models/_search`: a page of models in id
    /// order, chosen by `match_all` or `ids` (a query on any other field
    /// would read every model).
    fn search(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let raw = read_body(
            req,
            DocKind::KnnModels.max_doc_bytes(),
            "the body is over the limit for a search",
        )?;
        let body: Value = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?
        };
        let number = |name: &str, default: usize| -> Result<usize, RestError> {
            match body.get(name).filter(|v| !v.is_null()) {
                Some(v) => v
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| bad(format!("[{name}] must be a whole number"))),
                None => req.param(name).map_or(Ok(default), |v| {
                    v.parse::<usize>()
                        .map_err(|_| bad(format!("[{name}] must be a number")))
                }),
            }
        };
        let (from, size) = (number("from", 0)?, number("size", 10)?.min(MAX_PAGE));
        let wanted = wanted_ids(body.get("query"))?;
        let mut marker = self.marker(req)?;
        let mut names = self.set()?.names(&mut marker).map_err(|e| failed(&e))?;
        if let Some(ids) = &wanted {
            names.retain(|n| ids.contains(n));
        }
        let mut doc = Self::doc_buffer(req)?;
        // ALLOC: one entry per hit, at most the page size, 100.
        let mut hits = Vec::new();
        for name in names.iter().skip(from).take(size) {
            if let Some(model) = self.stored(name, &mut marker, &mut doc)? {
                hits.push(json!({
                    "_index": INDEX, "_id": name, "_score": 1.0, "_source": model,
                }));
            }
        }
        let out = json!({
            "took": 0, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {"total": {"value": names.len(), "relation": "eq"}, "max_score": 1.0, "hits": hits},
        });
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}

/// The ids a search names: `None` for `match_all` or no query.
fn wanted_ids(query: Option<&Value>) -> Result<Option<Vec<String>>, RestError> {
    let Some(query) = query else { return Ok(None) };
    let one = query
        .as_object()
        .filter(|m| m.len() == 1)
        .ok_or_else(|| bad("a query must be one object with one query type"))?;
    match one.iter().next() {
        Some((kind, _)) if kind == "match_all" => Ok(None),
        Some((kind, body)) if kind == "ids" => Ok(Some(
            body.get("values")
                .and_then(Value::as_array)
                .map(|v| {
                    v.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        )),
        Some((kind, _)) => Err(bad(format!(
            "query [{kind}] is not served: it would read every model, so only match_all and ids are"
        ))),
        None => Err(bad("empty query")),
    }
}

type Handler = fn(&KnnModelService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// The model routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<KnnModelService>) -> Result<Router, RouteError> {
    let table: [(Method, &'static str, Handler); 4] = [
        (
            Method::Get,
            "/_plugins/_knn/models/{id}",
            KnnModelService::get,
        ),
        (
            Method::Delete,
            "/_plugins/_knn/models/{id}",
            KnnModelService::delete,
        ),
        (
            Method::Get,
            "/_plugins/_knn/models/_search",
            KnnModelService::search,
        ),
        (
            Method::Post,
            "/_plugins/_knn/models/_search",
            KnnModelService::search,
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
