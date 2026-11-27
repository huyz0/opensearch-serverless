//! The `rare_terms` aggregation of a forwarded search:
//! `RareTermsAggregationBuilder`'s own fields as `innerWriteTo` writes them,
//! and the shard's `StringRareTerms` or `LongRareTerms` answering it, as
//! `InternalMappedRareTerms.doWriteTo` writes them, with the terms that were
//! too common in `SetBackedScalingCuckooFilter`'s set mode.

use super::Refusal;
use super::aggs::write_format;
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most terms the filter holds as a set (`RareTermsAggregator`'s
/// threshold); one more and OpenSearch's filter turns into cuckoo filters,
/// which this node does not write.
pub const FILTER_THRESHOLD: usize = 10_000;

/// The filter's capacity (`SetBackedScalingCuckooFilter`'s default).
const FILTER_CAPACITY: i32 = 1_000_000;

/// The filter's false-positive rate: the builder's `precision` is not on the
/// wire, so the shard's default.
const FILTER_FPP: f64 = 0.001;

/// What a `rare_terms` asks beyond its field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RareRequest {
    /// `max_doc_count`.
    pub max_doc_count: i64,
}

/// Reads the fields after a `rare_terms` builder's values-source fields.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or an `include` or `exclude`.
pub fn read(input: &mut Input<'_>) -> Result<RareRequest, Refusal> {
    if input.bool("an include or exclude")? {
        return Err(Refusal::Unsupported("an include or exclude"));
    }
    Ok(RareRequest {
        max_doc_count: input.vlong("max doc count")?,
    })
}

/// Writes `r` after a `rare_terms` builder's values-source fields.
///
/// # Errors
///
/// [`WireError`] past the writer's limit or for a negative count.
pub fn write(r: &RareRequest, out: &mut Output<'_>) -> Result<(), WireError> {
    out.bool(false)?; // include and exclude
    out.vlong(r.max_doc_count)
}

/// The keys of a shard's buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RareKeys {
    /// `StringRareTerms`: the terms.
    Text(Vec<String>),
    /// `LongRareTerms`: the numbers.
    Long(Vec<i64>),
}

impl RareKeys {
    fn len(&self) -> usize {
        match self {
            Self::Text(v) => v.len(),
            Self::Long(v) => v.len(),
        }
    }
}

/// A shard's `StringRareTerms` or `LongRareTerms`.
#[derive(Debug, Clone, PartialEq)]
pub struct RareAnswer {
    /// `max_doc_count`.
    pub max_doc_count: i64,
    /// The buckets' keys, by count then key.
    pub keys: RareKeys,
    /// The buckets' document counts, one a key.
    pub counts: Vec<i64>,
    /// The hashes the filter holds (`MurmurHash3.murmur64` of a number,
    /// the first half of `hash128` of a term): the terms held by more than
    /// `max_doc_count` documents.
    pub excluded: Vec<i64>,
}

/// The buckets of the table of the `HashSet<Long>` the filter keeps its hashes
/// in (found from what 3.10.0 writes: 16,384, which at most
/// [`FILTER_THRESHOLD`] hashes never fill to a resize).
const SET_TABLE: usize = 16_384;

/// The order `java.util.HashSet<Long>` lists `hashes`, which were added in
/// the order given: by bucket of its table, insertion order within a bucket.
fn set_order(hashes: &[i64]) -> Vec<i64> {
    let bucket = |v: i64| -> usize {
        // ARITH: none; shifts, masks and a truncation to Java's 32-bit hash.
        let bits = u64::from_ne_bytes(v.to_ne_bytes());
        let h = ((bits ^ (bits >> 32)) & 0xFFFF_FFFF) as u32;
        let h = h ^ (h >> 16);
        usize::try_from(h).unwrap_or(0) & SET_TABLE.saturating_sub(1)
    };
    // ALLOC: one entry a hash, at most FILTER_THRESHOLD.
    let mut listed = hashes.to_vec();
    listed.sort_by_key(|v| bucket(*v));
    listed
}

impl RareAnswer {
    /// The answer's writeable name.
    pub fn name(&self) -> &'static str {
        match self.keys {
            RareKeys::Text(_) => "srareterms",
            RareKeys::Long(_) => "lrareterms",
        }
    }

    /// `InternalRareTerms.doWriteTo`, `InternalMappedRareTerms`'s format,
    /// buckets and filter.
    ///
    /// # Errors
    ///
    /// [`WireError`] past the writer's limit, or for more terms in the filter
    /// than it holds as a set.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        if self.excluded.len() > FILTER_THRESHOLD {
            return Err(WireError::malformed(
                "a rare_terms filter",
                "more terms than the filter holds as a set",
            ));
        }
        out.byte(2)?; // the order: `_count`, ascending
        out.vlong(self.max_doc_count)?;
        write_format(out, false)?;
        out.count(self.keys.len())?;
        for (i, count) in self.counts.iter().enumerate() {
            out.vlong(*count)?;
            out.count(0)?; // sub-aggregations
            match &self.keys {
                RareKeys::Text(terms) => {
                    out.bytes_reference(terms.get(i).map_or(&[][..], |t| t.as_bytes()))?;
                }
                RareKeys::Long(keys) => {
                    out.bytes(&keys.get(i).copied().unwrap_or(0).to_be_bytes())?;
                }
            }
        }
        // `SetBackedScalingCuckooFilter.writeTo`, in set mode.
        out.count(FILTER_THRESHOLD)?;
        out.bool(true)?;
        out.vint(FILTER_CAPACITY)?;
        out.bytes(&FILTER_FPP.to_bits().to_be_bytes())?;
        out.count(self.excluded.len())?;
        for hash in set_order(&self.excluded) {
            out.zlong(hash)?;
        }
        Ok(())
    }
}
