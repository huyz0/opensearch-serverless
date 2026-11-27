//! A `range` result written as `InternalRange.doXContentBody` writes it, at
//! OpenSearch 3.10.0.

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::bucket::range::{RangeBucket, RangeResult};
use crate::aggs::bucket::sub_values::write_subs;
use crate::aggs::metric::date_text::write_date_of_double;
use crate::aggs::metric::double::write_java_double;
use crate::aggs::partial::Format;
use crate::date_pattern::DatePattern;

/// `format.format(value)`: a date field's bound as text, any other as the
/// double.
fn write_bound(
    out: &mut dyn Write,
    format: Format,
    pattern: Option<&DatePattern>,
    value: f64,
) -> io::Result<()> {
    match (format, pattern) {
        (Format::Raw | Format::Double, _) => write_java_double(out, value),
        // ARITH: none; a double is cast to a long first, as `format(double)` does.
        #[allow(clippy::cast_possible_truncation)]
        (Format::Date, Some(p)) => p.write(out, value as i64, 0),
        (Format::Date, None) => write_date_of_double(out, value),
    }
}

/// The key of `bucket`: the one asked for, else `from-to` with `*` for an
/// unbounded end (`InternalRange.Bucket.generateKey`).
pub fn key_of(
    format: Format,
    pattern: Option<&DatePattern>,
    bucket: &RangeBucket,
) -> io::Result<Vec<u8>> {
    // ALLOC: one key a bucket, bounded by the request's ranges.
    let mut key = Vec::new();
    if let Some(asked) = &bucket.key {
        key.extend_from_slice(asked.as_bytes());
        return Ok(key);
    }
    if bucket.from.is_infinite() {
        key.push(b'*');
    } else {
        write_bound(&mut key, format, pattern, bucket.from)?;
    }
    key.push(b'-');
    if bucket.to.is_infinite() {
        key.push(b'*');
    } else {
        write_bound(&mut key, format, pattern, bucket.to)?;
    }
    Ok(key)
}

/// A comma before every field but the first.
fn separate(out: &mut dyn Write, first: &mut bool) -> io::Result<()> {
    if !*first {
        out.write_all(b",")?;
    }
    *first = false;
    Ok(())
}

/// Writes one bound as `"name":<double>` and, for a date, `"name_as_string"`.
fn write_end(
    out: &mut dyn Write,
    first: &mut bool,
    format: Format,
    pattern: Option<&DatePattern>,
    name: &str,
    value: f64,
) -> io::Result<()> {
    separate(out, first)?;
    write!(out, "\"{name}\":")?;
    write_java_double(out, value)?;
    if format == Format::Date {
        write!(out, ",\"{name}_as_string\":\"")?;
        // ALLOC: one date text, bounded by the format.
        let mut text = Vec::new();
        write_bound(&mut text, format, pattern, value)?;
        out.write_all(&text)?;
        out.write_all(b"\"")?;
    }
    Ok(())
}

/// Writes the body of a `range`: its buckets, in an array or, when keyed,
/// an object.
pub fn write_range(out: &mut dyn Write, format: Format, result: &RangeResult) -> io::Result<()> {
    out.write_all(if result.keyed {
        b"\"buckets\":{"
    } else {
        b"\"buckets\":["
    })?;
    for (i, bucket) in result.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        let key =
            String::from_utf8_lossy(&key_of(format, result.format.as_ref(), bucket)?).into_owned();
        let mut first = true;
        // The keyed form names the object by the key; the other writes it.
        if result.keyed {
            write_json_str(out, &key)?;
            out.write_all(b":{")?;
        } else {
            out.write_all(b"{\"key\":")?;
            write_json_str(out, &key)?;
            first = false;
        }
        if !bucket.from.is_infinite() {
            write_end(
                out,
                &mut first,
                format,
                result.format.as_ref(),
                "from",
                bucket.from,
            )?;
        }
        if !bucket.to.is_infinite() {
            write_end(
                out,
                &mut first,
                format,
                result.format.as_ref(),
                "to",
                bucket.to,
            )?;
        }
        separate(out, &mut first)?;
        write!(out, "\"doc_count\":{}", bucket.doc_count)?;
        write_subs(out, &result.sub_meta, result.subs.get(i))?;
        out.write_all(b"}")?;
    }
    out.write_all(if result.keyed { b"}" } else { b"]" })
}
