//! The sub-aggregations of the bucket aggregations that run them on one
//! shard, by kind of bucket: fixed ones ([`fixed_subs`]) and ones keyed by the
//! data ([`keyed_subs`]).

use std::collections::HashMap;

use lucene_aggs::bucket_aggs::{AggNode, AggResult};
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::{fixed_subs, keyed_subs};
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Value;
use crate::search::ShardError;

/// Whether `agg` is a bucket aggregation that runs sub-aggregations here.
pub fn is_bucket(agg: &MetricAgg) -> bool {
    fixed_subs::is_fixed(agg) || keyed_subs::is_keyed(agg)
}

/// Puts the nodes of `agg`'s sub-aggregations under `node`, the engine's
/// node for `agg` (any other kind of node is left as it is).
pub fn attach<S: BlobStore + 'static>(
    node: &mut AggNode,
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, std::sync::Arc<GlobalOrds>>,
) -> Result<(), ShardError> {
    fixed_subs::attach(node, agg, shard, ords)?;
    keyed_subs::attach(node, agg, shard, ords)
}

/// The shard-level result of the bucket aggregation `agg` ([`is_bucket`])
/// from the engine's `result`, with its buckets' sub-aggregation values.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    if keyed_subs::is_keyed(agg) {
        keyed_subs::value(agg, shard, result)
    } else {
        fixed_subs::value(agg, shard, result)
    }
}
