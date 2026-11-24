//! The `composite` aggregation's request and shard-level result
//! (`CompositeAggregationBuilder`, `InternalComposite` at OpenSearch 3.10.0),
//! and the reduce of results across shards (`InternalComposite.reduce`).

use std::cmp::Ordering;

use lucene_aggs::bucket_aggs::DateRounding;

use crate::aggs::bucket::histogram::java_compare;
use crate::aggs::bucket::running;
use crate::aggs::partial::Value;

/// What a `composite` source reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SourceKind {
    /// `terms`: the value itself.
    Terms,
    /// `histogram`: `floor(v / interval) * interval`.
    Histogram {
        /// The width of a bucket.
        interval: f64,
    },
    /// `date_histogram`: a date rounded to its interval's start, as a long.
    DateHistogram {
        /// The rounding, in the zone and with the offset asked for.
        rounding: DateRounding,
    },
}

/// `MissingOrder`: where a source's missing value sorts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingOrder {
    /// Before every value.
    First,
    /// After every value.
    Last,
    /// First ascending, last descending.
    Default,
}

/// One source of a `composite`.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceSpec {
    /// The source's name, its key in a bucket's key.
    pub name: String,
    /// The field it reads.
    pub field: String,
    /// What it makes of a value.
    pub kind: SourceKind,
    /// `order: desc`.
    pub desc: bool,
    /// Documents without a value get a `null` key.
    pub missing_bucket: bool,
    /// Where the `null` key sorts.
    pub missing_order: MissingOrder,
}

/// An `after` value as the body wrote it.
#[derive(Clone, Debug, PartialEq)]
pub enum AfterValue {
    /// `null`.
    Null,
    /// A string.
    Str(String),
    /// A number, as written.
    Number(String),
}

/// What a `composite` aggregation asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeSpec {
    /// The sources, in request order.
    pub sources: Vec<SourceSpec>,
    /// How many buckets to answer.
    pub size: usize,
    /// `after`: one entry a source, by name, in the order written.
    pub after: Option<Vec<(String, AfterValue)>>,
}

/// One source's value in a bucket's key.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyPart {
    /// The missing bucket.
    Missing,
    /// A long (or a date's milliseconds).
    Long(i64),
    /// A double.
    Double(f64),
    /// A keyword's term.
    Term(Vec<u8>),
}

/// What one shard (or one reduce) holds for a `composite`.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeResult {
    /// What the aggregation asked for.
    pub spec: CompositeSpec,
    /// `(key, doc count)` in key order.
    pub buckets: Vec<(Vec<KeyPart>, u64)>,
}

impl SourceSpec {
    /// `SingleDimensionValuesSource.compare` of two source values: the
    /// missing value by `missing_order`, values by the source's order.
    fn compare(&self, a: &KeyPart, b: &KeyPart) -> Ordering {
        let missing_first = match self.missing_order {
            MissingOrder::First => true,
            MissingOrder::Last => false,
            MissingOrder::Default => !self.desc,
        };
        let natural = match (a, b) {
            (KeyPart::Missing, KeyPart::Missing) => return Ordering::Equal,
            (KeyPart::Missing, _) => {
                return if missing_first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (_, KeyPart::Missing) => {
                return if missing_first {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (KeyPart::Long(x), KeyPart::Long(y)) => x.cmp(y),
            (KeyPart::Double(x), KeyPart::Double(y)) => java_compare(*x, *y),
            (KeyPart::Term(x), KeyPart::Term(y)) => x.cmp(y),
            // Keys of one source are one kind.
            _ => Ordering::Equal,
        };
        if self.desc {
            natural.reverse()
        } else {
            natural
        }
    }
}

/// `CompositeKey` order: source by source.
fn compare_keys(sources: &[SourceSpec], a: &[KeyPart], b: &[KeyPart]) -> Ordering {
    for ((s, x), y) in sources.iter().zip(a).zip(b) {
        let o = s.compare(x, y);
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// `InternalComposite.reduce`: `None` unless there is a part, every part is
/// a `composite`, and the answer is within the bucket limit.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&CompositeResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Composite(c) => Some(c.as_ref()),
            _ => None,
        })
        // ALLOC: as above.
        .collect();
    let results = results?;
    let reduced = reduce_results(&results)?;
    if !running::add(reduced.buckets.len()) {
        return None;
    }
    // ALLOC: one box a `composite`, bounded by the request's.
    Some(Value::Composite(Box::new(reduced)))
}

/// The reduce of `parts`, which are not empty: equal keys add, the buckets
/// are in key order and cut at `size`.
pub fn reduce_results(parts: &[&CompositeResult]) -> Option<CompositeResult> {
    let spec = parts.first()?.spec.clone();
    let total: usize = parts.iter().map(|p| p.buckets.len()).sum();
    // ALLOC: one entry a bucket of the shards' results, at most `size` a shard.
    let mut all: Vec<(&[KeyPart], u64)> = Vec::with_capacity(total);
    for part in parts {
        all.extend(part.buckets.iter().map(|(k, c)| (k.as_slice(), *c)));
    }
    all.sort_by(|a, b| compare_keys(&spec.sources, a.0, b.0));
    // ALLOC: one entry a bucket of the answer, at most `size`.
    let mut buckets: Vec<(Vec<KeyPart>, u64)> = Vec::with_capacity(spec.size.min(total));
    for (key, count) in all {
        match buckets.last_mut() {
            Some(last) if compare_keys(&spec.sources, &last.0, key) == Ordering::Equal => {
                last.1 = last.1.saturating_add(count);
            }
            _ => {
                if buckets.len() >= spec.size {
                    break;
                }
                // ALLOC: one key a bucket of the answer, as above.
                buckets.push((key.to_vec(), count));
            }
        }
    }
    Some(CompositeResult { spec, buckets })
}
