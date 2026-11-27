//! A `filters` result written as `InternalFilters.doXContentBody` writes it
//! (a `filter` as `InternalFilter.doXContentBody` does),
//! at OpenSearch 3.10.0.

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::bucket::filters::{FilterKind, FiltersResult};
use crate::aggs::bucket::sub_values::write_subs;

/// Writes the body of a `filters`: its buckets, in an array or, when keyed,
/// an object named by the keys.
pub fn write_filters(out: &mut dyn Write, result: &FiltersResult) -> io::Result<()> {
    // `InternalFilter.doXContentBody`: the count, with no buckets around it.
    if result.single {
        let count = result.buckets.first().map_or(0, |b| b.1);
        write!(out, "\"doc_count\":{count}")?;
        return write_subs(out, &result.sub_meta, result.subs.first());
    }
    out.write_all(if result.keyed {
        b"\"buckets\":{"
    } else {
        b"\"buckets\":["
    })?;
    // An `adjacency_matrix` writes the buckets that hold documents.
    let skip_empty = result.kind == FilterKind::AdjacencyMatrix;
    let mut first = true;
    for (i, (key, count)) in result.buckets.iter().enumerate() {
        if skip_empty && *count == 0 {
            continue;
        }
        if !first {
            out.write_all(b",")?;
        }
        first = false;
        if result.keyed {
            write_json_str(out, key)?;
            out.write_all(b":")?;
        }
        if skip_empty {
            out.write_all(b"{\"key\":")?;
            write_json_str(out, key)?;
            write!(out, ",\"doc_count\":{count}")?;
        } else {
            write!(out, "{{\"doc_count\":{count}")?;
        }
        write_subs(out, &result.sub_meta, result.subs.get(i))?;
        out.write_all(b"}")?;
    }
    out.write_all(if result.keyed { b"}" } else { b"]" })
}
