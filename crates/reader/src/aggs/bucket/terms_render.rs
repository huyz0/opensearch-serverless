//! A `terms` result written as `LongTerms.doXContentBody` writes it
//! (`InternalTerms.doXContentCommon`), at OpenSearch 3.10.0.

use std::io::{self, Write};

use crate::aggs::bucket::terms::{TermsResult, reduce_results};
use crate::aggs::metric::Format;
use crate::aggs::metric::date_text::write_date;

/// Writes the body of the reduce of the one shard result `result`: the
/// error bound, the documents not in a bucket and the buckets, a date
/// field's keys also as text.
pub fn write_terms(out: &mut dyn Write, format: Format, result: &TermsResult) -> io::Result<()> {
    let reduced = reduce_results(&[result]);
    write!(
        out,
        "\"doc_count_error_upper_bound\":{},\"sum_other_doc_count\":{},\"buckets\":[",
        reduced.doc_count_error, reduced.other_doc_count
    )?;
    for (i, (key, count)) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write!(out, "{{\"key\":{key}")?;
        if format == Format::Date {
            out.write_all(b",\"key_as_string\":\"")?;
            write_date(out, *key)?;
            out.write_all(b"\"")?;
        }
        write!(out, ",\"doc_count\":{count}}}")?;
    }
    out.write_all(b"]")
}
