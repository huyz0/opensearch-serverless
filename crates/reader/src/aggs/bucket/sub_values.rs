//! The sub-aggregation values of a bucket aggregation's buckets: each
//! bucket's list of values (one a sub-aggregation, in request order), their
//! reduce across shards and their part of the response, shared by every
//! bucket aggregation that holds sub-aggregations.

use std::collections::HashMap;
use std::hash::Hash;
use std::io::{self, Write};

use crate::aggs::metric::hash_order::hash_map_order;
use crate::aggs::metric::render::write_body;
use crate::aggs::metric::typed::write_key;
use crate::aggs::partial::{self, Format, Value};

/// The name and format of each sub-aggregation, in request order, and one
/// list of values (one a sub-aggregation) a bucket.
pub type SubValues = (Vec<(String, Format)>, Vec<Vec<Value>>);

/// The reduce of one sub-aggregation's `parts`, one value a shard that
/// holds the bucket.
pub fn reduce_sub(parts: &[Value]) -> Value {
    if parts.iter().any(|p| matches!(p, Value::TopHits(_))) {
        // Merging hits needs them fetched and their shard known: the
        // coordinator takes every shard's result for the bucket.
        // ALLOC: one result a shard that holds the bucket, bounded by the shards.
        let all = parts
            .iter()
            .filter_map(|p| match p {
                Value::TopHits(top) => Some((**top).clone()),
                _ => None,
            })
            // ALLOC: one result a shard that holds the bucket.
            .collect();
        return Value::TopHitsParts(all);
    }
    partial::reduce(parts)
        .or_else(|| parts.first().cloned())
        .unwrap_or(Value::ValueCount(0))
}

/// The reduce of the sub-aggregation values of `buckets` buckets that every
/// shard holds, bucket for bucket: `parts` is one `subs` a shard. Empty
/// when no shard has a sub-aggregation.
pub fn reduce_fixed(parts: &[&[Vec<Value>]], buckets: usize) -> Vec<Vec<Value>> {
    let wanted = parts
        .iter()
        .flat_map(|p| p.iter())
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    if wanted == 0 {
        // ALLOC: empty.
        return Vec::new();
    }
    // ALLOC: one list a bucket, bounded by the request's buckets.
    (0..buckets)
        .map(|at| {
            // ALLOC: one value a sub-aggregation, bounded by the request's.
            (0..wanted)
                .map(|sub| {
                    // ALLOC: one value a shard, bounded by the shards that answered.
                    let values: Vec<Value> = parts
                        .iter()
                        .filter_map(|p| p.get(at).and_then(|list| list.get(sub)))
                        .cloned()
                        // ALLOC: as above.
                        .collect();
                    reduce_sub(&values)
                })
                // ALLOC: as above.
                .collect()
        })
        // ALLOC: one list a bucket, as above.
        .collect()
}

/// The sub-aggregations whose `values` a bucket holds, each as
/// `,"name":{..}`, in the order `Aggregations.toXContent` lists them.
pub fn write_subs(
    out: &mut dyn Write,
    meta: &[(String, Format)],
    values: Option<&Vec<Value>>,
) -> io::Result<()> {
    let Some(values) = values else {
        return Ok(());
    };
    // ALLOC: one name a sub-aggregation, bounded by the request's.
    let names: Vec<&str> = meta.iter().map(|(n, _)| n.as_str()).collect();
    for i in hash_map_order(&names) {
        let (Some((name, format)), Some(value)) = (meta.get(i), values.get(i)) else {
            continue;
        };
        out.write_all(b",")?;
        write_key(out, name, value, *format)?;
        out.write_all(b":{")?;
        write_body(out, *format, value)?;
        out.write_all(b"}")?;
    }
    Ok(())
}

/// The sub-aggregations of a bucket aggregation whose buckets come from the
/// data (a `histogram`), kept beside its `buckets`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BucketSubs {
    /// The name and format of each sub-aggregation, in request order.
    pub meta: Vec<(String, Format)>,
    /// What each sub-aggregation holds for a bucket with no document: the
    /// value of a bucket a reduce fills in.
    pub empty: Vec<Value>,
    /// Each bucket's values, one list a bucket in the order of `buckets`;
    /// empty when there are no sub-aggregations.
    pub lists: Vec<Vec<Value>>,
}

/// The reduce of the sub-aggregations of buckets named by a key: `parts` is
/// each shard's keys with its `subs` (one list a key), and `keys` the keys
/// the reduce keeps, in order. A key no shard holds (a bucket a reduce
/// filled) gets the empty value.
pub fn reduce_keyed<K: Copy + Eq + Hash>(parts: &[(&[K], &BucketSubs)], keys: &[K]) -> BucketSubs {
    let Some((_, first)) = parts.iter().find(|(_, s)| !s.meta.is_empty()) else {
        return BucketSubs::default();
    };
    let wanted = first.meta.len();
    // ALLOC: one list a sub-aggregation a distinct key, at most the buckets
    // the shards hold.
    let mut by_key: HashMap<K, Vec<Vec<Value>>> = HashMap::new();
    for (part_keys, subs) in parts {
        for (key, list) in part_keys.iter().zip(&subs.lists) {
            // ALLOC: one list a sub-aggregation a distinct key, as above.
            let slot = by_key
                .entry(*key)
                .or_insert_with(|| vec![Vec::new(); wanted]);
            for (values, value) in slot.iter_mut().zip(list) {
                // ALLOC: one value a sub-aggregation a shard holding the key.
                values.push(value.clone());
            }
        }
    }
    BucketSubs {
        // ALLOC: one entry a sub-aggregation, bounded by the request's.
        meta: first.meta.clone(),
        // ALLOC: as above.
        empty: first.empty.clone(),
        // ALLOC: one list a kept bucket, bounded by the bucket limit.
        lists: keys
            .iter()
            .map(|key| match by_key.get(key) {
                // ALLOC: one value a sub-aggregation a kept bucket.
                Some(slot) => slot.iter().map(|values| reduce_sub(values)).collect(),
                // ALLOC: as above.
                None => first.empty.clone(),
            })
            // ALLOC: as above.
            .collect(),
    }
}
