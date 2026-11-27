//! The result of a `multi_terms` (`InternalMultiTerms` at OpenSearch 3.10.0):
//! one shard's buckets, the reduce across shards (`InternalTerms.reduce`) and
//! the response body.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, Write};

use crate::aggs::bucket::running;
use crate::aggs::bucket::terms::{TermsSpec, compare, shard_error};
use crate::aggs::partial::Value;

/// One term of a bucket's key: a keyword's text, or a number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MultiPart {
    /// A keyword's term.
    Text(String),
    /// A long.
    Long(i64),
}

/// A bucket's key: one part a term of the aggregation.
pub type MultiKey = Vec<MultiPart>;

/// What one shard (or one reduce) holds for a `multi_terms`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiResult {
    /// What the aggregation asked for.
    pub spec: TermsSpec,
    /// The documents of the buckets not kept.
    pub other_doc_count: u64,
    /// `docCountError`: -1 when unbounded.
    pub doc_count_error: i64,
    /// `(key, doc count)`: by the order of the spec.
    pub buckets: Vec<(MultiKey, u64)>,
    /// Set by a reduce: the buckets are the answer, to write as they are.
    pub reduced: bool,
}

impl MultiResult {
    /// A shard's result from every combination of terms it counted: those
    /// under `shard_min_doc_count` go (their documents join the others), the
    /// best `shard_size` stay by the order, and the documents of the rest join
    /// the others (`MultiTermsAggregator.buildAggregations`).
    pub fn of_shard(spec: &TermsSpec, mut counted: Vec<(MultiKey, u64)>) -> Self {
        let mut other = 0u64;
        counted.retain(|(_, count)| {
            let kept = *count >= spec.shard_min_doc_count;
            if !kept {
                other = other.saturating_add(*count);
            }
            kept
        });
        counted.sort_by(|a, b| compare(spec.order, (&a.0, a.1), (&b.0, b.1)));
        for (_, count) in counted.iter().skip(spec.shard_size) {
            other = other.saturating_add(*count);
        }
        counted.truncate(spec.shard_size);
        Self {
            // ALLOC: one spec a `multi_terms`, bounded by the request's.
            spec: spec.clone(),
            other_doc_count: other,
            doc_count_error: 0,
            buckets: counted,
            reduced: false,
        }
    }
}

/// `InternalTerms.reduce` as a final reduce: `None` unless there is a part
/// and every part is a `multi_terms`.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&MultiResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Multi(m) => Some(m.as_ref()),
            _ => None,
        })
        // ALLOC: one reference a shard, as above.
        .collect();
    let results = results?;
    if results.is_empty() {
        return None;
    }
    let reduced = reduce_results(&results);
    if !running::add(reduced.buckets.len()) {
        return None;
    }
    // ALLOC: one box a `multi_terms`, bounded by the request's.
    Some(Value::Multi(Box::new(reduced)))
}

/// The final reduce of `parts`, which are not empty: equal keys add, the
/// buckets under `min_doc_count` go, the rest are ordered and cut at `size`,
/// the cut ones' documents joining `sum_other_doc_count`. It holds one entry
/// a distinct key of the parts.
pub fn reduce_results(parts: &[&MultiResult]) -> MultiResult {
    let spec = parts.first().map(|p| p.spec.clone()).unwrap_or_default();
    let mut other = 0u64;
    let mut sum_error = 0i64;
    // ALLOC: one entry a distinct key of the shards' buckets.
    let mut merged: BTreeMap<&MultiKey, u64> = BTreeMap::new();
    for part in parts {
        other = other.saturating_add(part.other_doc_count);
        let error = shard_error(&part.spec, &part.buckets, part.doc_count_error);
        sum_error = if sum_error == -1 || error == -1 {
            -1
        } else {
            sum_error.saturating_add(error)
        };
        for (key, count) in &part.buckets {
            let slot = merged.entry(key).or_insert(0);
            *slot = slot.saturating_add(*count);
        }
    }
    // ALLOC: one entry a bucket that passes `min_doc_count`.
    let mut kept: Vec<(&MultiKey, u64)> = merged
        .into_iter()
        .filter(|(_, count)| *count >= spec.min_doc_count)
        // ALLOC: as above.
        .collect();
    kept.sort_by(|a, b| compare(spec.order, (a.0, a.1), (b.0, b.1)));
    for &(_, count) in kept.iter().skip(spec.size) {
        other = other.saturating_add(count);
    }
    kept.truncate(spec.size);
    let doc_count_error = if sum_error == -1 || parts.len() != 1 {
        sum_error
    } else {
        0
    };
    MultiResult {
        spec,
        other_doc_count: other,
        doc_count_error,
        // ALLOC: one key a kept bucket, at most `size`.
        buckets: kept.into_iter().map(|(k, c)| (k.clone(), c)).collect(),
        reduced: true,
    }
}

/// Writes the body of `result` as the response has it: the error bound, the
/// documents not in a bucket and the buckets, each with its key as a list
/// and as one text (`InternalMultiTerms.doXContentBody`).
pub fn write_multi(out: &mut dyn Write, result: &MultiResult) -> io::Result<()> {
    let single;
    let shown = if result.reduced {
        result
    } else {
        single = reduce_results(&[result]);
        &single
    };
    write!(
        out,
        "\"doc_count_error_upper_bound\":{},\"sum_other_doc_count\":{},\"buckets\":[",
        shown.doc_count_error, shown.other_doc_count
    )?;
    for (i, (key, count)) in shown.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key\":[")?;
        // ALLOC: one text a bucket, at most `size`.
        let mut joined = String::new();
        for (at, part) in key.iter().enumerate() {
            if at > 0 {
                out.write_all(b",")?;
                joined.push('|');
            }
            match part {
                MultiPart::Text(text) => {
                    oss_rest::write_json_str(out, text)?;
                    joined.push_str(text);
                }
                MultiPart::Long(n) => {
                    write!(out, "{n}")?;
                    write!(joined, "{n}").map_err(io::Error::other)?;
                }
            }
        }
        out.write_all(b"],\"key_as_string\":")?;
        oss_rest::write_json_str(out, &joined)?;
        write!(out, ",\"doc_count\":{count}}}")?;
    }
    out.write_all(b"]")
}
