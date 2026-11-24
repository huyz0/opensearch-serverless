//! A `composite` result written as `CompositeAggregation.toXContentFragment`
//! writes it, at OpenSearch 3.10.0.

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::bucket::composite::{CompositeResult, KeyPart, reduce_results};
use crate::aggs::bucket::sub_values::write_subs;
use crate::aggs::metric::double::write_java_double;

/// Writes the body of the reduce of the one shard result `result`:
/// `after_key` (the last bucket's key, when there is a bucket) and the
/// buckets.
pub fn write_composite(out: &mut dyn Write, result: &CompositeResult) -> io::Result<()> {
    let reduced = reduce_results(&[result])
        .ok_or_else(|| io::Error::other("a composite without a result"))?;
    let names: Vec<&str> = reduced
        .spec
        .sources
        .iter()
        .map(|s| s.name.as_str())
        // ALLOC: one name a source, bounded by the request's.
        .collect();
    if let Some((last, _)) = reduced.buckets.last() {
        out.write_all(b"\"after_key\":")?;
        write_key(out, &names, last)?;
        out.write_all(b",")?;
    }
    out.write_all(b"\"buckets\":[")?;
    for (i, (key, count)) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key\":")?;
        write_key(out, &names, key)?;
        write!(out, ",\"doc_count\":{count}")?;
        write_subs(out, &reduced.subs.meta, reduced.subs.lists.get(i))?;
        out.write_all(b"}")?;
    }
    out.write_all(b"]")
}

/// A key as an object of the sources' values.
fn write_key(out: &mut dyn Write, names: &[&str], key: &[KeyPart]) -> io::Result<()> {
    out.write_all(b"{")?;
    for (i, (name, part)) in names.iter().zip(key).enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_json_str(out, name)?;
        out.write_all(b":")?;
        match part {
            KeyPart::Missing => out.write_all(b"null")?,
            KeyPart::Long(v) => write!(out, "{v}")?,
            KeyPart::Double(v) => write_java_double(out, *v)?,
            KeyPart::Term(t) => write_json_str(out, &String::from_utf8_lossy(t))?,
        }
    }
    out.write_all(b"}")
}
