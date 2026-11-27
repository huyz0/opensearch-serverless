//! An `auto_date_histogram` result written as
//! `InternalAutoDateHistogram.doXContentBody` writes it, at OpenSearch 3.10.0.

use std::io::{self, Write};

use crate::aggs::bucket::auto_date::AutoDateResult;
use crate::aggs::bucket::date_histogram_render::write_zoned;
use crate::aggs::partial::Format;

/// Writes the body of the reduce of the one shard result `result`: its
/// buckets, a date field's keys also as text, then the `interval`.
pub fn write_auto_date(
    out: &mut dyn Write,
    format: Format,
    result: &AutoDateResult,
) -> io::Result<()> {
    // A shard's result is reduced as it is written; a reduced one is written as it is.
    let again;
    let reduced = if result.reduced {
        result
    } else {
        again = crate::aggs::bucket::auto_date::reduce_results(&[result])
            .ok_or_else(|| io::Error::other("no buckets to reduce"))?;
        &again
    };
    let zone = reduced.spec.rounding(reduced.idx);
    let pattern = reduced.spec.format.as_ref();
    out.write_all(b"\"buckets\":[")?;
    for (i, (key, count)) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{")?;
        if format == Format::Date {
            out.write_all(b"\"key_as_string\":\"")?;
            write_zoned(out, *key, &zone, pattern)?;
            out.write_all(b"\",")?;
        }
        write!(out, "\"key\":{key},\"doc_count\":{count}}}")?;
    }
    write!(out, "],\"interval\":\"{}\"", reduced.interval())
}
