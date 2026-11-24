//! The shard-level result of a `terms` over a keyword field (`StringTerms`
//! at OpenSearch 3.10.0), its reduce across shards (`InternalTerms.reduce`)
//! and its response body (`StringTerms.doXContentBody`).

use std::io::{self, Write};

use crate::aggs::bucket::running;
use crate::aggs::bucket::sub_order;
use crate::aggs::bucket::sub_values::{reduce_sub, write_subs};
use crate::aggs::bucket::terms::{TermsSpec, bucket_errors, compare, shard_error};
use crate::aggs::partial::{Format, Value};

/// What one shard (or one reduce) holds for a keyword `terms`.
#[derive(Clone, Debug, PartialEq)]
pub struct KeywordTermsResult {
    /// What the aggregation asked for.
    pub spec: TermsSpec,
    /// The documents of the buckets not kept.
    pub other_doc_count: u64,
    /// `docCountError`: -1 when unbounded.
    pub doc_count_error: i64,
    /// `(term, doc count)`: by term as the order lists terms (ascending
    /// unless ordered by key descending) from a shard, by the order from a
    /// reduce.
    pub buckets: Vec<(String, u64)>,
    /// Each bucket's `doc_count_error_upper_bound`, as for a numeric
    /// `terms`.
    pub bucket_errors: Vec<i64>,
    /// The name and format of each sub-aggregation, in request order.
    pub sub_meta: Vec<(String, Format)>,
    /// Each bucket's sub-aggregation values, one list a bucket in the order
    /// of `buckets`, each in the order of `sub_meta`; empty lists when there
    /// are no sub-aggregations.
    pub subs: Vec<Vec<Value>>,
}

impl KeywordTermsResult {
    /// Drops the buckets under the spec's `shard_min_doc_count`, once the
    /// sub-aggregation lists are in place; their documents join the others.
    pub fn apply_shard_min(&mut self) {
        let dropped = super::terms::drop_below(
            self.spec.shard_min_doc_count,
            &mut self.buckets,
            &mut self.subs,
        );
        self.other_doc_count = self.other_doc_count.saturating_add(dropped);
    }
}

/// `InternalTerms.reduce` as a final reduce: `None` unless there is a part
/// and every part is a keyword `terms`.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&KeywordTermsResult>> = parts
        .iter()
        .map(|p| match p {
            Value::KeywordTerms(t) => Some(t.as_ref()),
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
    // ALLOC: one box a `terms`, bounded by the request's.
    Some(Value::KeywordTerms(Box::new(reduced)))
}

/// The final reduce of `parts`, which are not empty, of one aggregation:
/// the numeric `terms`' reduce over terms. It holds one entry a distinct
/// term of the parts.
pub fn reduce_results(parts: &[&KeywordTermsResult]) -> KeywordTermsResult {
    let spec = parts.first().map(|p| p.spec.clone()).unwrap_or_default();
    let mut other = 0u64;
    let mut sum_error = 0i64;
    // ALLOC: one error a part, bounded by the shards.
    let mut part_errors = Vec::with_capacity(parts.len());
    // ALLOC: one entry a distinct term of the shards' buckets.
    let mut merged: std::collections::BTreeMap<&str, (u64, Vec<Vec<Value>>)> =
        std::collections::BTreeMap::new();
    let wanted = spec_subs(parts);
    for part in parts {
        other = other.saturating_add(part.other_doc_count);
        let error = shard_error(&part.spec, &part.buckets, part.doc_count_error);
        part_errors.push(error);
        sum_error = if sum_error == -1 || error == -1 {
            -1
        } else {
            sum_error.saturating_add(error)
        };
        for (at, (term, count)) in part.buckets.iter().enumerate() {
            // ALLOC: one list a sub-aggregation a distinct term, bounded by the request's.
            let slot = merged
                .entry(term.as_str())
                .or_insert_with(|| (0, vec![Vec::new(); wanted]));
            slot.0 = slot.0.saturating_add(*count);
            for (list, value) in slot
                .1
                .iter_mut()
                .zip(part.subs.get(at).into_iter().flatten())
            {
                // ALLOC: one value a sub-aggregation a shard holding the term.
                list.push(value.clone());
            }
        }
    }
    // ALLOC: one entry a bucket that passes `min_doc_count`.
    let mut kept: Vec<(&str, u64, Vec<Vec<Value>>)> = merged
        .into_iter()
        .filter(|(_, (count, _))| *count >= spec.min_doc_count)
        // ALLOC: as above.
        .map(|(term, (count, lists))| (term, count, lists))
        // ALLOC: as above.
        .collect();
    if let Some(order) = &spec.sub_order {
        let target = sub_order::target(
            order,
            parts
                .first()
                .into_iter()
                .flat_map(|p| p.sub_meta.iter().map(|m| m.0.as_str())),
        );
        // ALLOC: one value a bucket, bounded by the buckets.
        let values: Vec<f64> = kept
            .iter()
            .map(|(_, _, lists)| {
                target
                    .as_ref()
                    .and_then(|(at, key)| {
                        lists
                            .get(*at)
                            .map(|l| sub_order::sort_value(&reduce_sub(l), key.as_deref()))
                    })
                    .unwrap_or(f64::NAN)
            })
            // ALLOC: as above.
            .collect();
        // ALLOC: one reference a bucket, as above.
        let terms: Vec<&str> = kept.iter().map(|k| k.0).collect();
        let rank = sub_order::ranked(order, &values, &terms);
        // ALLOC: one slot a bucket, as above.
        let mut slots: Vec<Option<_>> = kept.into_iter().map(Some).collect();
        // ALLOC: one entry a bucket, as above.
        kept = rank
            .into_iter()
            .filter_map(|at| slots.get_mut(at).and_then(Option::take))
            // ALLOC: as above.
            .collect();
    } else {
        kept.sort_by(|a, b| compare(spec.order, (&a.0, a.1), (&b.0, b.1)));
    }
    for &(_, count, _) in kept.iter().skip(spec.size) {
        other = other.saturating_add(count);
    }
    kept.truncate(spec.size);
    let shown = parts
        .iter()
        .zip(&part_errors)
        .map(|(p, e)| {
            // ALLOC: one key a shard bucket, at most `shard_size` a shard.
            let keys = p.buckets.iter().map(|b| b.0.as_str()).collect();
            (keys, p.bucket_errors.as_slice(), *e)
        })
        // ALLOC: one entry a shard, bounded by the shards.
        .collect::<Vec<_>>();
    // ALLOC: one key a kept bucket, at most `size`.
    let kept_keys: Vec<&str> = kept.iter().map(|k| k.0).collect();
    let bucket_errors = bucket_errors(spec.show_error, sum_error, &shown, &kept_keys);
    let doc_count_error = if sum_error == -1 || parts.len() != 1 {
        sum_error
    } else {
        0
    };
    KeywordTermsResult {
        spec,
        other_doc_count: other,
        doc_count_error,
        bucket_errors,
        // ALLOC: one term a kept bucket, at most `size`.
        buckets: kept.iter().map(|(k, c, _)| ((*k).to_owned(), *c)).collect(),
        sub_meta: parts
            .first()
            .map(|p| p.sub_meta.clone())
            .unwrap_or_default(),
        subs: kept
            .into_iter()
            // ALLOC: one value a sub-aggregation a kept bucket.
            .map(|(_, _, lists)| lists.iter().map(|l| reduce_sub(l)).collect())
            // ALLOC: one list a kept bucket, at most `size`.
            .collect(),
    }
}

/// How many sub-aggregations `parts` ask for.
fn spec_subs(parts: &[&KeywordTermsResult]) -> usize {
    parts.iter().map(|p| p.sub_meta.len()).max().unwrap_or(0)
}

/// Writes the body of the reduce of the one shard result `result`: the
/// error bound, the documents not in a bucket and the buckets.
pub fn write_keyword_terms(out: &mut dyn Write, result: &KeywordTermsResult) -> io::Result<()> {
    let reduced = reduce_results(&[result]);
    write!(
        out,
        "\"doc_count_error_upper_bound\":{},\"sum_other_doc_count\":{},\"buckets\":[",
        reduced.doc_count_error, reduced.other_doc_count
    )?;
    for (i, (term, count)) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key\":")?;
        oss_rest::write_json_str(out, term)?;
        write!(out, ",\"doc_count\":{count}")?;
        if reduced.spec.show_error {
            let error = reduced.bucket_errors.get(i).copied().unwrap_or(0);
            write!(out, ",\"doc_count_error_upper_bound\":{error}")?;
        }
        write_subs(out, &reduced.sub_meta, reduced.subs.get(i))?;
        out.write_all(b"}")?;
    }
    out.write_all(b"]")
}
