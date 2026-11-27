//! A `terms` result written as `LongTerms.doXContentBody` writes it
//! (`InternalTerms.doXContentCommon`), at OpenSearch 3.10.0.

use std::io::{self, Write};

use crate::aggs::bucket::sub_values::write_subs;
use crate::aggs::bucket::terms::{TermsResult, reduce_results};
use crate::aggs::metric::date_text::write_date;
use crate::aggs::partial::Format;

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
        if format == Format::Double {
            out.write_all(b"{\"key\":")?;
            let d = crate::aggs::bucket::terms_collect::double_of_key(*key);
            out.write_all(crate::template::java_double(d).as_bytes())?;
        } else {
            write!(out, "{{\"key\":{key}")?;
        }
        if format == Format::Date {
            out.write_all(b",\"key_as_string\":\"")?;
            write_date(out, *key)?;
            out.write_all(b"\"")?;
        }
        write!(out, ",\"doc_count\":{count}")?;
        if reduced.spec.show_error {
            let error = reduced.bucket_errors.get(i).copied().unwrap_or(0);
            write!(out, ",\"doc_count_error_upper_bound\":{error}")?;
        }
        write_subs(out, &reduced.subs.meta, reduced.subs.lists.get(i))?;
        out.write_all(b"}")?;
    }
    out.write_all(b"]")
}
