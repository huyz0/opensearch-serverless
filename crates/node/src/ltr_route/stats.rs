//! `GET /_ltr/_stats` and `POST /_ltr/_clearcache`: what this node's model
//! cache has counted and holds, the stores' sizes, and forgetting a store's
//! cached objects.

use oss_meta::MetaError;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::models::{CacheCounts, StoreModels};
use super::{DEFAULT, LtrService, Object, failed, index_name};
use crate::route_util::refuse;

/// The stats a request may name.
const STATS: &[&str] = &[
    "status",
    "stores",
    "cache",
    "request_total_count",
    "request_error_count",
];

fn counts(c: CacheCounts) -> Value {
    json!({
        "eviction_count": c.evictions,
        "hit_count": c.hits,
        "miss_count": c.misses,
        "entry_count": c.entries,
        "memory_usage_in_bytes": c.bytes,
    })
}

impl LtrService {
    /// The cache this node resolves through, when learning to rank is served.
    fn cache(&self) -> Result<&StoreModels, RestError> {
        self.cache
            .as_deref()
            .ok_or_else(|| refuse(500, "ltr_store_failure", "the model cache is not installed"))
    }

    /// `GET /_ltr/_stats[/{stat}]`: this node's stats.
    pub(crate) fn stats(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let wanted = req.path_param("stat").map(std::borrow::Cow::into_owned);
        if let Some(stat) = &wanted
            && !STATS.contains(&stat.as_str())
        {
            return Err(refuse(
                400,
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!("unknown stat [{stat}]"),
            ));
        }
        let want = |name: &str| wanted.as_deref().is_none_or(|w| w == name);
        let mut body = Map::new();
        body.insert(
            "_nodes".to_owned(),
            json!({"total": 1, "successful": 1, "failed": 0}),
        );
        if want("status") {
            body.insert("status".to_owned(), json!("green"));
        }
        if want("stores") {
            let mut marker = self.marker(req, 0)?;
            let mut stores = Map::new();
            for name in self.store_names(&mut marker)? {
                let mut count = |object: Object| match self.set(&name, object)?.names(&mut marker) {
                    Ok(n) => Ok(n.len()),
                    Err(MetaError::NoSuchSet) => Ok(0),
                    Err(e) => Err(failed(&e)),
                };
                let featuresets = count(Object::FeatureSet)?;
                let models = count(Object::Model)?;
                stores.insert(
                    index_name(&name),
                    json!({"model_count": models, "featureset_count": featuresets, "status": "green"}),
                );
            }
            body.insert("stores".to_owned(), Value::Object(stores));
        }
        let stats = self.cache()?.stats();
        let mut node = Map::new();
        if want("cache") {
            node.insert(
                "cache".to_owned(),
                json!({
                    "feature": counts(CacheCounts::default()),
                    "featureset": counts(stats.featureset),
                    "model": counts(stats.model),
                }),
            );
        }
        if want("request_total_count") {
            node.insert("request_total_count".to_owned(), json!(stats.requests));
        }
        if want("request_error_count") {
            node.insert("request_error_count".to_owned(), json!(stats.errors));
        }
        if !node.is_empty() {
            body.insert("nodes".to_owned(), json!({ self.node_id.as_str(): node }));
        }
        resp.send(200, JSON, Value::Object(body).to_string().as_bytes())
    }

    /// `POST /_ltr[/{store}]/_clearcache`: forgets this node's cached
    /// objects of the store.
    pub(crate) fn clear_cache(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let store = Self::store_of(req)?;
        let mut marker = self.marker(req, 0)?;
        if !self.store_names(&mut marker)?.contains(&store) {
            return Err(super::missing(&store));
        }
        self.cache()?.clear_store(&store);
        let body = json!({
            "_nodes": {"total": 1, "successful": 1, "failed": 0},
            "nodes": { self.node_id.as_str(): {"name": self.node_id} },
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// The stores that exist: the registry's, and the default one when its
    /// marker does.
    pub(super) fn store_names(&self, marker: &mut [u8]) -> Result<Vec<String>, RestError> {
        let mut names = self.registry()?.names(marker).map_err(|e| failed(&e))?;
        match self.set(DEFAULT, Object::FeatureSet)?.names(marker) {
            Ok(_) => names.insert(0, DEFAULT.to_owned()),
            Err(MetaError::NoSuchSet) => {}
            Err(e) => return Err(failed(&e)),
        }
        Ok(names)
    }
}
