//! A `filters` result written as `InternalFilters.doXContentBody` writes it
//! (a `filter` as `InternalFilter.doXContentBody` does),
//! at OpenSearch 3.10.0.

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::bucket::filters::FiltersResult;

/// Writes the body of a `filters`: its buckets, in an array or, when keyed,
/// an object named by the keys.
pub fn write_filters(out: &mut dyn Write, result: &FiltersResult) -> io::Result<()> {
    // `InternalFilter.doXContentBody`: the count, with no buckets around it.
    if result.single {
        let count = result.buckets.first().map_or(0, |b| b.1);
        return write!(out, "\"doc_count\":{count}");
    }
    out.write_all(if result.keyed {
        b"\"buckets\":{"
    } else {
        b"\"buckets\":["
    })?;
    for (i, (key, count)) in result.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        if result.keyed {
            write_json_str(out, key)?;
            out.write_all(b":")?;
        }
        write!(out, "{{\"doc_count\":{count}}}")?;
    }
    out.write_all(if result.keyed { b"}" } else { b"]" })
}
