//! `/_ltr`: the learning to rank plugin's feature store, served natively
//! over document sets (ADR 0071).
//!
//! A store is a set of feature sets and a set of models, each a
//! [`DocSet`] under `ltr/<store>/`, and a name in the `ltr/stores/` registry.
//! The default store is `_default_`, shown as the plugin's `.ltrstore`. A put
//! is the marker's read, the content's write and the marker's swap; a read
//! the marker's read and the content's; nothing lists.

mod objects;
mod routes;
mod stores;

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::{DocKind, MAX_DOC_BYTES, MAX_DOC_NAME_BYTES};
use oss_meta::{DocSet, MetaError, Put};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use crate::route_util::{read_body, refuse};
use objects::{FeatureSet, Model};
pub use routes::routes;

/// The default store's directory name.
const DEFAULT: &str = "_default_";
/// What a model put's definition may add to the bytes a request holds: the
/// recorded tokens of a definition cost at most this many times its size.
const PARSE_FACTOR: usize = 32;

/// Which kind of object a route reads or writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Object {
    FeatureSet,
    Model,
}

impl Object {
    fn kind(self) -> DocKind {
        match self {
            Self::FeatureSet => DocKind::LtrFeatureSets,
            Self::Model => DocKind::LtrModels,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FeatureSet => "featureset",
            Self::Model => "model",
        }
    }
}

/// Serves the store routes over one deployment's object store.
pub struct LtrService {
    store: Arc<dyn BlobStore>,
    base: String,
    buffer_bytes: usize,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn missing(store: &str) -> RestError {
    refuse(
        404,
        "index_not_found_exception",
        // ALLOC: error path; one message.
        format!("no such index [{}]", index_name(store)),
    )
}

/// The index name the plugin shows for a store.
fn index_name(store: &str) -> String {
    if store == DEFAULT {
        ".ltrstore".to_owned()
    } else {
        format!(".ltrstore_{store}")
    }
}

/// A store name that is one safe path segment.
fn valid_store(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_DOC_NAME_BYTES
        && !name.starts_with(['_', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl LtrService {
    /// A service over the stores under `base` in `store`; a request holds up
    /// to `buffer_bytes` for a marker.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, buffer_bytes: usize) -> Self {
        Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base, once.
            base: base.to_owned(),
            buffer_bytes,
        }
    }

    fn registry(&self) -> Result<DocSet, RestError> {
        DocSet::new(Arc::clone(&self.store), &self.base, DocKind::LtrStores).map_err(|e| failed(&e))
    }

    fn set(&self, store: &str, object: Object) -> Result<DocSet, RestError> {
        // ALLOC: one key prefix, once a request.
        let base = format!("{}ltr/{store}/", self.base);
        DocSet::new_created(Arc::clone(&self.store), &base, object.kind()).map_err(|e| failed(&e))
    }

    /// The store named by the request, `_default_` without one.
    fn store_of(req: &Request<'_>) -> Result<String, RestError> {
        match req.path_param("store") {
            None => Ok(DEFAULT.to_owned()),
            Some(s) if valid_store(&s) => Ok(s.into_owned()),
            Some(s) => Err(bad(format!("invalid store name [{s}]"))),
        }
    }

    fn marker(&self, req: &mut Request<'_>, extra: usize) -> Result<Vec<u8>, RestError> {
        req.body()
            .reserve(self.buffer_bytes.saturating_add(extra))?;
        // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    fn doc_buffer(&self, req: &mut Request<'_>, object: Object) -> Result<Vec<u8>, RestError> {
        let bytes = object.kind().max_doc_bytes();
        req.body().reserve(bytes)?;
        // ALLOC: one document buffer a request, the kind's bound, reserved above.
        Ok(vec![0u8; bytes])
    }

    /// Reads a stored object, `None` when the store has no such object.
    fn read(
        &self,
        store: &str,
        object: Object,
        name: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Value>, RestError> {
        let set = self.set(store, object)?;
        match set.get(name, marker, doc) {
            Ok(Some(n)) => serde_json::from_slice(doc.get(..n).unwrap_or_default())
                .map(Some)
                .map_err(|_| refuse(500, "ltr_store_failure", "a stored object is unreadable")),
            Ok(None) => Ok(None),
            Err(MetaError::NoSuchSet) => Err(missing(store)),
            Err(e) => Err(failed(&e)),
        }
    }

    fn id(object: Object, name: &str) -> String {
        format!("{}-{name}", object.label())
    }

    /// `GET /_ltr/_featureset/{name}` and `/_ltr/_model/{name}`.
    pub(crate) fn get(
        &self,
        object: Object,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (store, name) = (Self::store_of(req)?, Self::name_of(req)?);
        let mut marker = self.marker(req, 0)?;
        let mut doc = self.doc_buffer(req, object)?;
        let found = self.read(&store, object, &name, &mut marker, &mut doc)?;
        let id = Self::id(object, &name);
        let (status, body) = match found {
            Some(source) => (
                200,
                json!({"_index": index_name(&store), "_id": id, "found": true, "_source": source}),
            ),
            None => (
                404,
                json!({"_index": index_name(&store), "_id": id, "found": false}),
            ),
        };
        resp.send(status, JSON, body.to_string().as_bytes())
    }

    fn name_of(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("name")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a name is required"))
    }

    /// `DELETE`.
    pub(crate) fn delete(
        &self,
        object: Object,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (store, name) = (Self::store_of(req)?, Self::name_of(req)?);
        let mut marker = self.marker(req, 0)?;
        let id = Self::id(object, &name);
        let gone = match self.set(&store, object)?.delete(&name, &mut marker) {
            Ok(g) => g,
            Err(MetaError::NoSuchSet) => return Err(missing(&store)),
            Err(e) => return Err(failed(&e)),
        };
        let (status, result) = if gone {
            (200, "deleted")
        } else {
            (404, "not_found")
        };
        let body = json!({"_index": index_name(&store), "_id": id, "result": result, "forced_refresh": true});
        resp.send(status, JSON, body.to_string().as_bytes())
    }

    /// Stores `doc` and answers the put's shape.
    fn store_doc(
        &self,
        object: Object,
        store: &str,
        name: &str,
        doc: &Value,
        marker: &mut [u8],
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = doc.to_string().into_bytes();
        let Put { version, replaced } = match self
            .set(store, object)?
            .put_counted(name, &bytes, false, marker)
        {
            Ok(p) => p,
            Err(MetaError::NoSuchSet) => return Err(missing(store)),
            Err(e) => return Err(failed(&e)),
        };
        let body = json!({
            "_index": index_name(store),
            "_id": Self::id(object, name),
            "_version": version,
            "result": if replaced { "updated" } else { "created" },
            "forced_refresh": true,
            "_shards": {"total": 1, "successful": 1, "failed": 0},
            "_seq_no": 0,
            "_primary_term": 1,
        });
        resp.send(
            if replaced { 200 } else { 201 },
            JSON,
            body.to_string().as_bytes(),
        )
    }

    fn body_json(req: &mut Request<'_>, object: Object) -> Result<Value, RestError> {
        let max = object.kind().max_doc_bytes();
        let body = read_body(req, max, "the body is over the limit for this object")?;
        serde_json::from_slice(&body).map_err(|e| bad(format!("invalid body: {e}")))
    }

    /// `PUT|POST /_ltr/_featureset/{name}`.
    pub(crate) fn put_featureset(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (store, name) = (Self::store_of(req)?, Self::name_of(req)?);
        let body = Self::body_json(req, Object::FeatureSet)?;
        let set = FeatureSet::from_put(&name, &body).map_err(bad)?;
        let mut marker = self.marker(req, 0)?;
        self.store_doc(
            Object::FeatureSet,
            &store,
            &name,
            &set.stored(),
            &mut marker,
            resp,
        )
    }

    /// `POST /_ltr/_featureset/{name}/_addfeatures`: the body's features
    /// join the set, which is made when absent.
    pub(crate) fn add_features(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let (store, name) = (Self::store_of(req)?, Self::name_of(req)?);
        let body = Self::body_json(req, Object::FeatureSet)?;
        let added = objects::features(body.get("features").unwrap_or(&Value::Null)).map_err(bad)?;
        let mut marker = self.marker(req, 0)?;
        let mut doc = self.doc_buffer(req, Object::FeatureSet)?;
        let mut set = match self.read(&store, Object::FeatureSet, &name, &mut marker, &mut doc)? {
            Some(stored) => FeatureSet::from_stored(&stored).map_err(bad)?,
            // ALLOC: one name, once.
            None => FeatureSet {
                name: name.clone(),
                features: Vec::new(),
            },
        };
        for feature in added {
            if set.features.iter().any(|f| f["name"] == feature["name"]) {
                return Err(bad(format!(
                    "feature {} is already in the set [{name}]",
                    feature["name"]
                )));
            }
            set.features.push(feature);
        }
        if set.features.len() > oss_reader::ltr::MAX_FEATURES {
            return Err(bad("a feature set holds at most 10000 features"));
        }
        self.store_doc(
            Object::FeatureSet,
            &store,
            &name,
            &set.stored(),
            &mut marker,
            resp,
        )
    }

    /// `PUT|POST /_ltr/_model/{name}` (the set embedded in the body) and
    /// `POST /_ltr/_featureset/{set}/_createmodel` (the set stored).
    pub(crate) fn put_model(
        &self,
        from_set: bool,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let store = Self::store_of(req)?;
        let path_name = if from_set {
            None
        } else {
            Some(Self::name_of(req)?)
        };
        let body = Self::body_json(req, Object::Model)?;
        let (model, embedded) = Model::from_body(path_name.as_deref(), &body).map_err(bad)?;
        // The recorded tokens of the definition, reserved before they are made.
        req.body()
            .reserve(model_bytes(&body).saturating_mul(PARSE_FACTOR))?;
        let mut marker = self.marker(req, 0)?;
        let set = if from_set {
            let set_name = Self::name_of(req)?;
            let mut doc = vec![0u8; MAX_DOC_BYTES];
            req.body().reserve(MAX_DOC_BYTES)?;
            let stored = self
                .read(&store, Object::FeatureSet, &set_name, &mut marker, &mut doc)?
                .ok_or_else(|| {
                    refuse(
                        404,
                        "resource_not_found_exception",
                        format!("feature set [{set_name}] does not exist"),
                    )
                })?;
            FeatureSet::from_stored(&stored).map_err(bad)?
        } else {
            let Some(embedded) = embedded else {
                return Err(bad("a model put needs its [feature_set]"));
            };
            FeatureSet::from_embedded(&embedded).map_err(bad)?
        };
        model.check(&set).map_err(bad)?;
        self.store_doc(
            Object::Model,
            &store,
            &model.name,
            &model.stored(&set),
            &mut marker,
            resp,
        )
    }

    /// `GET /_ltr/_featureset` and `/_ltr/_model`: a page of the objects in
    /// name order, optionally those with a `prefix`.
    pub(crate) fn page(
        &self,
        object: Object,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let store = Self::store_of(req)?;
        let prefix = req
            .param("prefix")
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default();
        let number = |name: &str, default: usize| -> Result<usize, RestError> {
            req.param(name).map_or(Ok(default), |v| {
                v.parse::<usize>()
                    .map_err(|_| bad(format!("[{name}] must be a number")))
            })
        };
        let (from, size) = (number("from", 0)?, number("size", 20)?.min(100));
        let mut marker = self.marker(req, 0)?;
        let names = match self.set(&store, object)?.names(&mut marker) {
            Ok(n) => n,
            Err(MetaError::NoSuchSet) => return Err(missing(&store)),
            Err(e) => return Err(failed(&e)),
        };
        let matching: Vec<&String> = names.iter().filter(|n| n.starts_with(&prefix)).collect();
        let mut doc = self.doc_buffer(req, object)?;
        // ALLOC: one entry per hit, at most the page size, 100.
        let mut hits = Vec::new();
        for name in matching.iter().skip(from).take(size) {
            if let Some(source) = self.read(&store, object, name, &mut marker, &mut doc)? {
                hits.push(json!({"_index": index_name(&store), "_id": Self::id(object, name), "_score": 1.0, "_source": source}));
            }
        }
        let body = json!({"took": 0, "timed_out": false, "hits": {
            "total": {"value": matching.len(), "relation": "eq"}, "max_score": 1.0, "hits": hits}});
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}

/// The bytes of a model body's definition.
fn model_bytes(body: &Value) -> usize {
    match body.pointer("/model/model/definition") {
        Some(Value::String(s)) => s.len(),
        Some(v) => v.to_string().len(),
        None => 0,
    }
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(400, "too_many_objects", e.to_string()),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "ltr_store_failure", other.to_string()),
    }
}
