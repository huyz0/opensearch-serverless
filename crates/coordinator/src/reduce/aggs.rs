//! Reducing each named metric aggregation across the shards that answered.

use oss_reader::aggs::partial::{Value, reduce_total};

/// Why a named aggregation could not be reduced.
#[derive(Clone, Debug, PartialEq)]
pub enum AggsError {
    /// The shards' values for `name` are not all one kind, or their sketches'
    /// precisions differ.
    Mismatched {
        /// The aggregation's name.
        name: String,
    },
    /// `name` is a `top_hits`, whose hits are merged with their shard known.
    TopHits {
        /// The aggregation's name.
        name: String,
    },
    /// The reduce made more buckets, over every level of one aggregation,
    /// than `search.max_buckets` allows.
    TooManyBuckets {
        /// The bucket count reached.
        made: usize,
    },
}

/// Each name's reduce over the shards that have it, names in the order they
/// first appear; `shards` holds each answering shard's aggregations in shard
/// order.
pub fn reduce_shards(shards: &[Vec<(String, Value)>]) -> Result<Vec<(String, Value)>, AggsError> {
    // ALLOC: one entry per distinct aggregation name, bounded by the request's aggregations.
    let mut names: Vec<&str> = Vec::new();
    for shard in shards {
        for (name, _) in shard {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
    }
    // ALLOC: one reduced value per name.
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let parts: Vec<Value> = shards
            .iter()
            .flat_map(|shard| shard.iter().filter(|(n, _)| n == name))
            .map(|(_, value)| value.clone())
            // ALLOC: at most one value per shard for this name.
            .collect();
        if parts.iter().any(|v| matches!(v, Value::TopHits(_))) {
            return Err(AggsError::TopHits { name: name.into() });
        }
        let value = reduce_total(&parts)
            .map_err(|made| AggsError::TooManyBuckets { made })?
            .ok_or_else(|| AggsError::Mismatched { name: name.into() })?;
        out.push((name.into(), value));
    }
    Ok(out)
}
