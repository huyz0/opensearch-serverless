//! The shard-level result of a `rare_terms` over a keyword field
//! (`StringRareTerms` at OpenSearch 3.10.0), its reduce across shards
//! (`InternalRareTerms.reduce`) and its response body.

use std::io::{self, Write};

use crate::aggs::bucket::running;
use crate::aggs::partial::Value;

/// What a `rare_terms` asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct RareSpec {
    /// The most documents a rare term is held by.
    pub max_doc_count: u64,
    /// The cuckoo filter's precision: read, and not used, as this node counts
    /// exactly.
    pub precision: f64,
}

impl Default for RareSpec {
    fn default() -> Self {
        Self {
            max_doc_count: 1,
            precision: 0.001,
        }
    }
}

/// What one shard (or one reduce) holds for a `rare_terms`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RareResult {
    /// What the aggregation asked for.
    pub spec: RareSpec,
    /// The terms held by at most `max_doc_count` documents, with their
    /// counts, by term.
    pub buckets: Vec<(String, u64)>,
    /// The terms held by more: what OpenSearch's filter remembers.
    pub excluded: Vec<String>,
}

/// `InternalRareTerms.reduce` as a final reduce: `None` unless there is a
/// part and every part is a `rare_terms`.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&RareResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Rare(r) => Some(r.as_ref()),
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
    // ALLOC: one box a `rare_terms`, bounded by the request's.
    Some(Value::Rare(Box::new(reduced)))
}

/// The final reduce of `parts`, which are not empty: counts added over the
/// shards, a term any shard excluded dropped, the terms over `max_doc_count`
/// dropped, the rest by count and then term.
pub fn reduce_results(parts: &[&RareResult]) -> RareResult {
    let spec = parts.first().map(|p| p.spec.clone()).unwrap_or_default();
    // ALLOC: one entry a distinct term of the shards' results.
    let mut merged: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    for part in parts {
        for (term, count) in &part.buckets {
            let slot = merged.entry(term.as_str()).or_insert(0);
            *slot = slot.saturating_add(*count);
        }
    }
    for part in parts {
        for term in &part.excluded {
            merged.remove(term.as_str());
        }
    }
    // ALLOC: one bucket a distinct term, as above.
    let mut buckets: Vec<(String, u64)> = merged
        .into_iter()
        .filter(|(_, count)| *count <= spec.max_doc_count)
        // ALLOC: one term a distinct term, as above.
        .map(|(term, count)| (term.to_owned(), count))
        // ALLOC: as above.
        .collect();
    buckets.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    RareResult {
        spec,
        buckets,
        // ALLOC: empty; a final reduce keeps no filter.
        excluded: Vec::new(),
    }
}

/// Writes the body of the reduce of the one shard result `result`: its
/// buckets.
pub fn write_rare(out: &mut dyn Write, result: &RareResult) -> io::Result<()> {
    let reduced = reduce_results(&[result]);
    out.write_all(b"\"buckets\":[")?;
    for (i, (term, count)) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key\":")?;
        oss_rest::write_json_str(out, term)?;
        write!(out, ",\"doc_count\":{count}}}")?;
    }
    out.write_all(b"]")
}
