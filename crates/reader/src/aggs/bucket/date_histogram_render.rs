//! A `date_histogram` result written as `InternalDateHistogram.doXContentBody`
//! writes it, at OpenSearch 3.10.0.

use std::io::{self, Write};

use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::date_histogram::{DateHistogramResult, reduce_results};
use crate::aggs::metric::date_text::write_date;
use crate::aggs::partial::Format;

/// A key as the field's date format prints it in the aggregation's zone:
/// the local time, then `Z` for a zero offset or `+hh:mm`.
fn write_zoned(out: &mut dyn Write, key: i64, zone_ms: i64) -> io::Result<()> {
    if zone_ms == 0 {
        return write_date(out, key);
    }
    // ALLOC: one date text, 30 bytes.
    let mut local = Vec::with_capacity(30);
    write_date(&mut local, key.saturating_add(zone_ms))?;
    local.pop();
    out.write_all(&local)?;
    let minutes = zone_ms.abs() / 60_000;
    write!(
        out,
        "{}{:02}:{:02}",
        if zone_ms < 0 { '-' } else { '+' },
        minutes / 60,
        minutes % 60
    )
}

/// Writes the body of the reduce of the one shard result `result`: its
/// buckets as an array, or an object keyed by each key's text when `keyed`,
/// a date field's keys also as text.
pub fn write_date_histogram(
    out: &mut dyn Write,
    format: Format,
    result: &DateHistogramResult,
) -> io::Result<()> {
    // ALLOC: error path; one message.
    let reduced = reduce_results(&[result], BUCKET_LIMIT)
        .map_err(|made| io::Error::other(format!("too many buckets: {made}")))?;
    let keyed = reduced.spec.keyed;
    let zone = reduced.spec.rounding.zone_ms;
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
            if format == Format::Date {
                write_zoned(out, *key, zone)?;
            } else {
                write!(out, "{key}")?;
            }
            out.write_all(b"\":")?;
        }
        out.write_all(b"{")?;
        if format == Format::Date {
            out.write_all(b"\"key_as_string\":\"")?;
            write_zoned(out, *key, zone)?;
            out.write_all(b"\",")?;
        }
        write!(out, "\"key\":{key},\"doc_count\":{count}}}")?;
    }
    out.write_all(if keyed { b"}" } else { b"]" })
}
