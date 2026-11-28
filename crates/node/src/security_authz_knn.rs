//! The k-NN plugin's routes and the actions its security plugin names for them.

use oss_rest::Method;

use super::{GET_POST, Rule};

pub(super) const RULES: &[(&[Method], &str, Rule)] = &[
    (
        &[Method::Get],
        "/_plugins/_knn/stats",
        Rule::Cluster("cluster:admin/knn_stats_action"),
    ),
    (
        &[Method::Get],
        "/_plugins/_knn/stats/{stat}",
        Rule::Cluster("cluster:admin/knn_stats_action"),
    ),
    (
        &[Method::Get],
        "/_plugins/_knn/{node}/stats",
        Rule::Cluster("cluster:admin/knn_stats_action"),
    ),
    (
        &[Method::Get],
        "/_plugins/_knn/{node}/stats/{stat}",
        Rule::Cluster("cluster:admin/knn_stats_action"),
    ),
    (
        &[Method::Get],
        "/_plugins/_knn/warmup/{index}",
        Rule::Cluster("cluster:admin/knn_warmup_action"),
    ),
    (
        &[Method::Get],
        "/_plugins/_knn/models/{id}",
        Rule::Cluster("cluster:admin/knn_get_model_action"),
    ),
    (
        &[Method::Delete],
        "/_plugins/_knn/models/{id}",
        Rule::Cluster("cluster:admin/knn_delete_model_action"),
    ),
    (
        GET_POST,
        "/_plugins/_knn/models/_search",
        Rule::Cluster("cluster:admin/knn_search_model_action"),
    ),
    (
        &[Method::Post],
        "/_plugins/_knn/models/{id}/_train",
        Rule::Cluster("cluster:admin/knn_training_model_action"),
    ),
    (
        &[Method::Post],
        "/_plugins/_knn/models/_train",
        Rule::Cluster("cluster:admin/knn_training_model_action"),
    ),
    (
        &[Method::Get],
        "/_plugins/_knn/clear_cache/{index}",
        Rule::Index("indices:admin/knn_clear_cache_action"),
    ),
];
