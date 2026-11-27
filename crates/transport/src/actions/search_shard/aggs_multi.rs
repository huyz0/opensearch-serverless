//! The `multi_terms` aggregation of a forwarded search:
//! `MultiTermsAggregationBuilder`'s own fields as `doWriteTo` writes them, and
//! the shard's `InternalMultiTerms` answering it, as `InternalTerms.doWriteTo`
//! and `InternalMultiTerms.Bucket.writeTo` write them.

use super::Refusal;
use super::aggs::write_format;
use super::aggs_terms::{OrderSpec, TermsRequest};
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most terms one aggregation may carry.
const MAX_TERMS: usize = 16;

/// `StreamOutput.writeGenericValue`'s type byte of a `BytesRef`.
const GENERIC_BYTES_REF: u8 = 21;

/// `StreamOutput.writeGenericValue`'s type byte of a `Long`.
const GENERIC_LONG: u8 = 2;

/// What a `multi_terms` asks: its terms' fields and its options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiRequest {
    /// The field of each term, in order.
    pub fields: Vec<String>,
    /// `size`, `shard_size`, `min_doc_count`, `shard_min_doc_count`, `order`
    /// and `show_term_doc_count_error`.
    pub options: TermsRequest,
}

fn none(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// Reads the fields after a `multi_terms` builder's name, sub-aggregations
/// and metadata.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or an option this node does not run.
pub fn read(input: &mut Input<'_>) -> Result<MultiRequest, Refusal> {
    let count = input.count(1, "multi terms")?;
    if count > MAX_TERMS {
        return Err(Refusal::TooLarge("multi terms"));
    }
    // ALLOC: one field name a term, at most MAX_TERMS.
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let field = input
            .optional_string("a term field")?
            .ok_or(Refusal::Unsupported("a term without a field"))?;
        if input.byte("a term missing value")? != 0xFF {
            return Err(Refusal::Unsupported("a term missing value"));
        }
        none(input, "a term script")?;
        none(input, "a term time zone")?;
        none(input, "a term value type")?;
        if input.optional_string("a term format")?.is_some() {
            return Err(Refusal::Unsupported("a term format"));
        }
        none(input, "a term include or exclude")?;
        fields.push(field);
    }
    // `BucketCountThresholds.writeTo`.
    let size = input.int("size")?;
    let shard_size = input.int("shard size")?;
    let min_doc_count = input.long("min doc count")?;
    let shard_min_doc_count = input.long("shard min doc count")?;
    none(input, "a collect mode")?;
    let order = OrderSpec::read(input)?;
    let show_error = input.bool("show term doc count error")?;
    Ok(MultiRequest {
        fields,
        options: TermsRequest {
            size,
            shard_size,
            min_doc_count,
            shard_min_doc_count,
            order,
            show_error,
        },
    })
}

/// Writes `r` after a `multi_terms` builder's name, sub-aggregations and
/// metadata.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write(r: &MultiRequest, out: &mut Output<'_>) -> Result<(), WireError> {
    out.count(r.fields.len())?;
    for field in &r.fields {
        out.optional_string(Some(field))?;
        out.byte(0xFF)?; // missing: null
        for _ in 0..5 {
            // script, time zone, value type hint, format (a null optional
            // string is the same byte), include and exclude
            out.bool(false)?;
        }
    }
    let t = &r.options;
    out.bytes(&t.size.to_be_bytes())?;
    out.bytes(&t.shard_size.to_be_bytes())?;
    out.bytes(&t.min_doc_count.to_be_bytes())?;
    out.bytes(&t.shard_min_doc_count.to_be_bytes())?;
    out.bool(false)?; // collect mode
    t.order.write(out)?;
    out.bool(t.show_error)
}

/// One term of a bucket's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiTerm {
    /// A keyword's term (a `BytesRef`).
    Text(String),
    /// A number (a `Long`).
    Long(i64),
}

/// A shard's `InternalMultiTerms`.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiAnswer {
    /// The order asked for, which is also the order of the buckets.
    pub order: OrderSpec,
    /// `size`.
    pub size: i32,
    /// `min_doc_count`.
    pub min_doc_count: i64,
    /// `docCountError`.
    pub doc_count_error: i64,
    /// The shard size.
    pub shard_size: i32,
    /// The documents of the buckets not kept.
    pub other_doc_count: i64,
    /// How many terms a key has.
    pub terms: usize,
    /// The buckets' keys.
    pub keys: Vec<Vec<MultiTerm>>,
    /// The buckets' document counts, one a key.
    pub counts: Vec<i64>,
}

impl MultiAnswer {
    /// `InternalTerms.doWriteTo` and `InternalMultiTerms.writeTermTypeInfoTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.order.write(out)?; // reduceOrder
        self.order.write(out)?;
        out.count(usize::try_from(self.size).unwrap_or(0))?;
        out.vlong(self.min_doc_count)?;
        out.zlong(self.doc_count_error)?;
        out.count(self.terms)?;
        for _ in 0..self.terms {
            write_format(out, false)?;
        }
        out.count(usize::try_from(self.shard_size).unwrap_or(0))?;
        out.bool(false)?; // show_term_doc_count_error
        out.vlong(self.other_doc_count)?;
        out.count(self.keys.len())?;
        for (key, count) in self.keys.iter().zip(&self.counts) {
            out.count(key.len())?;
            for term in key {
                match term {
                    MultiTerm::Text(text) => {
                        out.byte(GENERIC_BYTES_REF)?;
                        out.bytes_reference(text.as_bytes())?;
                    }
                    MultiTerm::Long(n) => {
                        out.byte(GENERIC_LONG)?;
                        out.bytes(&n.to_be_bytes())?;
                    }
                }
            }
            out.vlong(*count)?;
            out.count(0)?; // sub-aggregations
        }
        Ok(())
    }
}
