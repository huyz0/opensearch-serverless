//! A `histogram` result written as `InternalHistogram.doXContentBody` writes
//! it, at OpenSearch 3.10.0.

use std::io::{self, Write};

use crate::aggs::bucket::sub_values::write_subs;

use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::histogram::{HistogramResult, reduce_results};
use crate::aggs::metric::date_text::write_date_of_double;
use crate::aggs::metric::double::write_java_double;
use crate::aggs::partial::Format;

/// Writes the body of the reduce of the one shard result `result`: its
/// buckets as an array, or an object keyed by each key's text when `keyed`,
/// a date field's keys also as text.
pub fn write_histogram(
    out: &mut dyn Write,
    format: Format,
    result: &HistogramResult,
) -> io::Result<()> {
    // ALLOC: error path; one message.
    let reduced = reduce_results(&[result], BUCKET_LIMIT)
        .map_err(|made| io::Error::other(format!("too many buckets: {made}")))?;
    let keyed = reduced.spec.keyed;
    out.write_all(if keyed {
        b"\"buckets\":{"
    } else {
        b"\"buckets\":["
    })?;
    for (i, (key, count)) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        if keyed {
            out.write_all(b"\"")?;
            write_text(out, format, *key)?;
            out.write_all(b"\":")?;
        }
        out.write_all(b"{")?;
        if format == Format::Date {
            out.write_all(b"\"key_as_string\":\"")?;
            write_date_of_double(out, *key)?;
            out.write_all(b"\",")?;
        }
        out.write_all(b"\"key\":")?;
        write_java_double(out, *key)?;
        write!(out, ",\"doc_count\":{count}")?;
        write_subs(out, &reduced.subs.meta, reduced.subs.lists.get(i))?;
        out.write_all(b"}")?;
    }
    out.write_all(if keyed { b"}" } else { b"]" })
}

/// A key as `DocValueFormat.format(double)` writes it.
fn write_text(out: &mut dyn Write, format: Format, key: f64) -> io::Result<()> {
    match format {
        Format::Raw | Format::Double => write_java_double(out, key),
        Format::Date => write_date_of_double(out, key),
    }
}
