//! `/_plugins/_knn/{stats,warmup,clear_cache}`: the k-NN plugin's node-level
//! calls, answered for this node (docs/specs/knn-plugin-api).
//!
//! Stats are the reader's process-wide `knn` counters, made no request. A
//! warmup opens each shard of the index as a search does and reads its
//! vector files in one prefetch. A clear closes this node's open shards of
//! the index, which returns their `open_indexes` charge; the next search
//! opens afresh.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::knn::stats;
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};
use serde_json::{Map, Value, json};

use super::{SearchService, open_shard::set_key, send};

/// The parameters these routes take.
const PARAMS: &[&str] = &["timeout"];

/// The stats a request may name, each with the value of a node that keeps no
/// native graph cache: the Lucene engine's graphs live in the segment files.
fn node_stats(node: &stats::Snapshot) -> Map<String, Value> {
    let mut m = Map::new();
    let mut put = |name: &str, v: Value| {
        m.insert(name.to_owned(), v);
    };
    for zero in [
        "eviction_count",
        "miss_count",
        "graph_memory_usage",
        "graph_memory_usage_percentage",
        "graph_index_requests",
        "graph_query_requests",
        "graph_query_errors",
        "hit_count",
        "load_success_count",
        "load_exception_count",
        "total_load_time",
        "script_compilations",
        "script_compilation_errors",
        "script_query_requests",
        "script_query_errors",
        "training_requests",
        "training_errors",
        "training_memory_usage",
        "training_memory_usage_percentage",
    ] {
        put(zero, json!(0));
    }
    put("cache_capacity_reached", json!(false));
    put("indexing_from_model_degraded", json!(false));
    put("indices_in_cache", json!({}));
    put("faiss_initialized", json!(false));
    put("nmslib_initialized", json!(false));
    put("lucene_initialized", json!(node.queries > 0));
    put("knn_query_requests", json!(node.queries));
    put("knn_query_with_filter_requests", json!(node.filtered));
    m
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// `GET /_plugins/_knn[/{node}]/stats[/{stat}]`.
    fn knn_stats(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        if let Some(node) = req.path_param("node")
            && !matches!(&*node, "_local" | "_all")
            && node != self.node_id
        {
            return Err(RestError::exception(
                404,
                oss_rest::Exception::new(
                    "failed_node_exception",
                    // ALLOC: error path; one message.
                    format!("Failed node [{node}]"),
                ),
            ));
        }
        let mut stats = node_stats(&stats::snapshot());
        if let Some(stat) = req.path_param("stat") {
            let Some(value) = stats.remove(&*stat) else {
                if matches!(&*stat, "circuit_breaker_triggered" | "model_index_status") {
                    return self.knn_stat_body(resp, Map::new());
                }
                return Err(RestError::illegal_argument(format!(
                    "Request contains unrecognized stat: [{stat}]"
                )));
            };
            let mut only = Map::new();
            only.insert(stat.into_owned(), value);
            return self.knn_stat_body(resp, only);
        }
        self.knn_stat_body(resp, stats)
    }

    fn knn_stat_body(
        &self,
        resp: &mut Response<'_>,
        node: Map<String, Value>,
    ) -> Result<(), RestError> {
        let body = json!({
            "_nodes": {"total": 1, "successful": 1, "failed": 0},
            "cluster_name": "opensearch",
            "circuit_breaker_triggered": false,
            "model_index_status": null,
            "nodes": { self.node_id.as_str(): node },
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// The shards of each index of the `{index}` list, opened.
    fn knn_shards(
        &self,
        req: &Request<'_>,
    ) -> Result<Vec<(String, u32)>, Box<oss_reader::failure::SearchError>> {
        let names = req.path_param("index").unwrap_or_default();
        let mut out = Vec::new();
        for index in names.split(',').filter(|n| !n.is_empty()) {
            self.open(index, 0)?;
            let count = self
                .lock()
                .peek(&set_key(index, 0))
                .map_or(1, |held| held.shard.meta.number_of_shards.max(1));
            out.extend((0..count).map(|shard| (index.to_owned(), shard)));
        }
        Ok(out)
    }

    /// `GET /_plugins/_knn/warmup/{index}`.
    fn knn_warmup(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let shards = match self.knn_shards(req) {
            Ok(shards) => shards,
            Err(e) => return send(resp, &e),
        };
        let mut ok = 0_usize;
        for (index, shard) in &shards {
            if self.open(index, *shard).is_ok() {
                let held = self.lock().peek(&set_key(index, *shard)).map(Arc::clone);
                if let Some(held) = held {
                    oss_reader::vector_warm::warm(&held.shard);
                    ok = ok.saturating_add(1);
                }
            }
        }
        shards_body(resp, shards.len(), ok)
    }

    /// `GET /_plugins/_knn/clear_cache/{index}`.
    fn knn_clear(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let shards = match self.knn_shards(req) {
            Ok(shards) => shards,
            Err(e) => return send(resp, &e),
        };
        // Taken out under the lock, dropped after it: a closed shard's
        // engine state and charge go with the last handle.
        let closed: Vec<_> = {
            let mut set = self.lock();
            shards
                .iter()
                .filter_map(|(index, shard)| set.release(&set_key(index, *shard)))
                .collect()
        };
        drop(closed);
        shards_body(resp, shards.len(), shards.len())
    }
}

fn shards_body(resp: &mut Response<'_>, total: usize, ok: usize) -> Result<(), RestError> {
    let body =
        json!({"_shards": {"total": total, "successful": ok, "failed": total.saturating_sub(ok)}});
    resp.send(200, JSON, body.to_string().as_bytes())
}

/// The routes of the plugin's node-level calls.
///
/// # Errors
///
/// A route registered twice.
pub fn routes<S: BlobStore + 'static>(
    router: Router,
    service: &Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for path in [
        "/_plugins/_knn/stats",
        "/_plugins/_knn/stats/{stat}",
        "/_plugins/_knn/{node}/stats",
        "/_plugins/_knn/{node}/stats/{stat}",
    ] {
        let service = Arc::clone(service);
        router = router.route(
            Method::Get,
            path,
            PARAMS,
            move |q: &mut Request<'_>, r: &mut Response<'_>| service.knn_stats(q, r),
        )?;
    }
    let warm = Arc::clone(service);
    let clear = Arc::clone(service);
    router
        .route(
            Method::Get,
            "/_plugins/_knn/warmup/{index}",
            PARAMS,
            move |q: &mut Request<'_>, r: &mut Response<'_>| warm.knn_warmup(q, r),
        )?
        .route(
            Method::Get,
            "/_plugins/_knn/clear_cache/{index}",
            PARAMS,
            move |q: &mut Request<'_>, r: &mut Response<'_>| clear.knn_clear(q, r),
        )
}
