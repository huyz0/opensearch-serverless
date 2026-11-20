//! The sub-aggregation values of a bucket aggregation's buckets: each
//! bucket's list of values (one a sub-aggregation, in request order), their
//! reduce across shards and their part of the response, shared by every
//! bucket aggregation that holds sub-aggregations.

use std::io::{self, Write};

use crate::aggs::metric::hash_order::hash_map_order;
use crate::aggs::metric::render::write_body;
use crate::aggs::partial::{self, Format, Value};

/// The name and format of each sub-aggregation, in request order, and one
/// list of values (one a sub-aggregation) a bucket.
pub type SubValues = (Vec<(String, Format)>, Vec<Vec<Value>>);

/// The reduce of one sub-aggregation's `parts`, one value a shard that
/// holds the bucket.
pub fn reduce_sub(parts: &[Value]) -> Value {
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
        oss_rest::write_json_str(out, name)?;
        out.write_all(b":{")?;
        write_body(out, *format, value)?;
        out.write_all(b"}")?;
    }
    Ok(())
}
