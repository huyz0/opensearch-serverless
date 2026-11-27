//! The shard-level result of a `significant_terms` over a keyword field
//! (`SignificantStringTerms` at OpenSearch 3.10.0), its heuristics
//! (`JLHScore`, `PercentageScore`), its reduce across shards
//! (`InternalSignificantTerms.reduce`) and its response body.

use std::io::{self, Write};

use crate::aggs::bucket::running;
use crate::aggs::metric::double::write_java_double;
use crate::aggs::partial::Value;

/// A significance heuristic this node scores with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Heuristic {
    /// `JLHScore`: the default.
    #[default]
    Jlh,
    /// `PercentageScore`.
    Percentage,
}

impl Heuristic {
    /// `getScore(subsetFreq, subsetSize, supersetFreq, supersetSize)`.
    pub fn score(
        self,
        subset_df: u64,
        subset_size: u64,
        superset_df: u64,
        superset_size: u64,
    ) -> f64 {
        match self {
            Heuristic::Jlh => {
                if subset_size == 0 || superset_size == 0 {
                    return 0.0;
                }
                // A foreground term the background lacks counts as one.
                let superset_df = superset_df.max(1);
                let subset = subset_df as f64 / subset_size as f64;
                let superset = superset_df as f64 / superset_size as f64;
                let absolute = subset - superset;
                if absolute <= 0.0 {
                    return 0.0;
                }
                absolute * (subset / superset)
            }
            Heuristic::Percentage => {
                if superset_df == 0 {
                    return 0.0;
                }
                subset_df as f64 / superset_df as f64
            }
        }
    }
}

/// What a `significant_terms` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignificantSpec {
    /// How many buckets the answer keeps.
    pub size: usize,
    /// What `shard_size` asked for or was suggested; a shard keeps `size`.
    pub shard_size: usize,
    /// The fewest documents a bucket holds.
    pub min_doc_count: u64,
    /// `shard_min_doc_count`, at most `min_doc_count`; not used by a shard.
    pub shard_min_doc_count: u64,
    /// How buckets are scored.
    pub heuristic: Heuristic,
}

impl Default for SignificantSpec {
    fn default() -> Self {
        Self {
            size: 10,
            shard_size: 25,
            min_doc_count: 3,
            shard_min_doc_count: 0,
            heuristic: Heuristic::Jlh,
        }
    }
}

/// One term with its foreground and background document counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigBucket {
    /// The term.
    pub term: String,
    /// Documents of the query's matches holding it (`subsetDf`).
    pub subset_df: u64,
    /// Documents of the background holding it (`supersetDf`).
    pub superset_df: u64,
}

/// What one shard (or one reduce) holds for a `significant_terms`.
#[derive(Clone, Debug, PartialEq)]
pub struct SignificantResult {
    /// What the aggregation asked for.
    pub spec: SignificantSpec,
    /// The query's matches (`subsetSize`).
    pub subset_size: u64,
    /// The background's documents (`supersetSize`).
    pub superset_size: u64,
    /// The kept buckets: by term from a shard, by score from a reduce.
    pub buckets: Vec<SigBucket>,
}

impl SignificantResult {
    fn score_of(&self, b: &SigBucket) -> f64 {
        self.spec.heuristic.score(
            b.subset_df,
            self.subset_size,
            b.superset_df,
            self.superset_size,
        )
    }

    /// A shard's selection: the best `size` by score of the buckets with at
    /// least `min_doc_count` documents, ties by term.
    pub fn select(&mut self) {
        let min = self.spec.min_doc_count;
        self.buckets.retain(|b| b.subset_df >= min);
        order_by_score(self);
        self.buckets.truncate(self.spec.size);
        self.buckets.sort_by(|a, b| a.term.cmp(&b.term));
    }
}

/// Sorts `r`'s buckets best score first, ties by term.
fn order_by_score(r: &mut SignificantResult) {
    // ALLOC: one score a bucket, bounded by the shard's candidates.
    let mut scored: Vec<(f64, SigBucket)> = std::mem::take(&mut r.buckets)
        .into_iter()
        .map(|b| (r.score_of(&b), b))
        // ALLOC: as above.
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.term.cmp(&b.1.term)));
    // ALLOC: one bucket a candidate, as above.
    r.buckets = scored.into_iter().map(|(_, b)| b).collect();
}

/// `InternalSignificantTerms.reduce` as a final reduce: `None` unless there
/// is a part and every part is a `significant_terms`.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&SignificantResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Significant(r) => Some(r.as_ref()),
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
    // ALLOC: one box a `significant_terms`, bounded by the request's.
    Some(Value::Significant(Box::new(reduced)))
}

/// The final reduce of `parts`, which are not empty: sizes and counts summed
/// over the shards, each term rescored with the global sizes.
pub fn reduce_results(parts: &[&SignificantResult]) -> SignificantResult {
    let spec = parts.first().map(|p| p.spec.clone()).unwrap_or_default();
    let subset_size = parts
        .iter()
        .fold(0u64, |n, p| n.saturating_add(p.subset_size));
    let superset_size = parts
        .iter()
        .fold(0u64, |n, p| n.saturating_add(p.superset_size));
    // ALLOC: one entry a distinct term of the shards' buckets.
    let mut merged: std::collections::BTreeMap<&str, (u64, u64)> =
        std::collections::BTreeMap::new();
    for part in parts {
        for b in &part.buckets {
            let slot = merged.entry(b.term.as_str()).or_insert((0, 0));
            slot.0 = slot.0.saturating_add(b.subset_df);
            slot.1 = slot.1.saturating_add(b.superset_df);
        }
    }
    let mut out = SignificantResult {
        // ALLOC: one spec a reduce, bounded by the request's.
        spec,
        subset_size,
        superset_size,
        // ALLOC: one bucket a distinct term, as above.
        buckets: merged
            .into_iter()
            .map(|(term, (subset_df, superset_df))| SigBucket {
                // ALLOC: one term a distinct term, as above.
                term: term.to_owned(),
                subset_df,
                superset_df,
            })
            // ALLOC: as above.
            .collect(),
    };
    let min = out.spec.min_doc_count;
    // ALLOC: one flag a bucket, bounded by the buckets.
    let keep: Vec<bool> = out
        .buckets
        .iter()
        .map(|b| b.subset_df >= min && out.score_of(b) > 0.0)
        // ALLOC: as above.
        .collect();
    let mut flags = keep.into_iter();
    out.buckets.retain(|_| flags.next().unwrap_or(false));
    order_by_score(&mut out);
    out.buckets.truncate(out.spec.size);
    out
}

/// Writes the body of the reduce of the one shard result `result`: the
/// sizes and the buckets.
pub fn write_significant(out: &mut dyn Write, result: &SignificantResult) -> io::Result<()> {
    let reduced = reduce_results(&[result]);
    write!(
        out,
        "\"doc_count\":{},\"bg_count\":{},\"buckets\":[",
        reduced.subset_size, reduced.superset_size
    )?;
    for (i, b) in reduced.buckets.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key\":")?;
        oss_rest::write_json_str(out, &b.term)?;
        write!(out, ",\"doc_count\":{},\"score\":", b.subset_df)?;
        write_java_double(out, reduced.score_of(b))?;
        write!(out, ",\"bg_count\":{}}}", b.superset_df)?;
    }
    out.write_all(b"]")
}
