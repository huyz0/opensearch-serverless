//! The `terms` aggregation of a forwarded search: `TermsAggregationBuilder`'s
//! own fields as `innerWriteTo` writes them, and the shard's `StringTerms` or
//! `LongTerms` answering it, as `InternalTerms.doWriteTo` writes them.

use super::Refusal;
use super::aggs::{AggValue, write_format, write_one};
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most buckets an order list or an answer may carry.
const MAX_ORDERS: usize = 16;

/// A bucket order that does not read a sub-aggregation (`InternalOrder`'s
/// ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// `_count`, descending.
    CountDesc,
    /// `_count`, ascending.
    CountAsc,
    /// `_key`, descending.
    KeyDesc,
    /// `_key`, ascending.
    KeyAsc,
}

impl Order {
    fn id(self) -> u8 {
        match self {
            Self::CountDesc => 1,
            Self::CountAsc => 2,
            Self::KeyDesc => 3,
            Self::KeyAsc => 4,
        }
    }

    fn from_id(id: u8) -> Option<Self> {
        [Self::CountDesc, Self::CountAsc, Self::KeyDesc, Self::KeyAsc]
            .into_iter()
            .find(|o| o.id() == id)
    }

    /// The `_count` or `_key` and the direction, as a request body writes
    /// them.
    pub fn json(self) -> (&'static str, &'static str) {
        match self {
            Self::CountDesc => ("_count", "desc"),
            Self::CountAsc => ("_count", "asc"),
            Self::KeyDesc => ("_key", "desc"),
            Self::KeyAsc => ("_key", "asc"),
        }
    }
}

/// A `BucketOrder`: one order, or a compound of them (`CompoundOrder`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderSpec {
    /// Whether it is written as a compound.
    pub compound: bool,
    /// The orders, in precedence.
    pub items: Vec<Order>,
}

impl OrderSpec {
    /// The order a `terms` has when none is asked for: by count descending,
    /// then by key ascending.
    pub fn is_default(&self) -> bool {
        self.compound && self.items == [Order::CountDesc, Order::KeyAsc]
    }

    fn read(input: &mut Input<'_>) -> Result<Self, Refusal> {
        let tag = input.byte("a bucket order")?;
        if tag != 0xFF {
            let order =
                Order::from_id(tag).ok_or(Refusal::Unsupported("an order by a sub-aggregation"))?;
            return Ok(Self {
                compound: false,
                // ALLOC: one order.
                items: vec![order],
            });
        }
        let n = input.count(1, "orders")?;
        if n > MAX_ORDERS {
            return Err(Refusal::TooLarge("orders"));
        }
        // ALLOC: one order each, at most MAX_ORDERS.
        let mut items = Vec::with_capacity(n);
        for _ in 0..n {
            let id = input.byte("a bucket order")?;
            items.push(
                Order::from_id(id).ok_or(Refusal::Unsupported("an order by a sub-aggregation"))?,
            );
        }
        Ok(Self {
            compound: true,
            items,
        })
    }

    fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        if !self.compound {
            return out.byte(self.items.first().map_or(1, |o| o.id()));
        }
        out.byte(0xFF)?;
        out.count(self.items.len())?;
        for o in &self.items {
            out.byte(o.id())?;
        }
        Ok(())
    }
}

/// What a `terms` asks beyond its field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermsRequest {
    /// `size`.
    pub size: i32,
    /// `shard_size`; -1 when none was asked for.
    pub shard_size: i32,
    /// `min_doc_count`.
    pub min_doc_count: i64,
    /// `shard_min_doc_count`.
    pub shard_min_doc_count: i64,
    /// `order`.
    pub order: OrderSpec,
    /// `show_term_doc_count_error`.
    pub show_error: bool,
}

fn none(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// Reads the fields after a `terms` builder's values-source fields.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or an option this node does not run.
pub fn read(input: &mut Input<'_>) -> Result<TermsRequest, Refusal> {
    // `BucketCountThresholds.writeTo`.
    let size = input.int("size")?;
    let shard_size = input.int("shard size")?;
    let min_doc_count = input.long("min doc count")?;
    let shard_min_doc_count = input.long("shard min doc count")?;
    none(input, "a collect mode")?;
    if input.optional_string("an execution hint")?.is_some() {
        return Err(Refusal::Unsupported("an execution hint"));
    }
    none(input, "an include or exclude")?;
    let order = OrderSpec::read(input)?;
    let show_error = input.bool("show term doc count error")?;
    Ok(TermsRequest {
        size,
        shard_size,
        min_doc_count,
        shard_min_doc_count,
        order,
        show_error,
    })
}

/// Writes `t` after a `terms` builder's values-source fields.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write(t: &TermsRequest, out: &mut Output<'_>) -> Result<(), WireError> {
    out.bytes(&t.size.to_be_bytes())?;
    out.bytes(&t.shard_size.to_be_bytes())?;
    out.bytes(&t.min_doc_count.to_be_bytes())?;
    out.bytes(&t.shard_min_doc_count.to_be_bytes())?;
    out.bool(false)?; // collect mode
    out.optional_string(None)?; // execution hint
    out.bool(false)?; // include and exclude
    t.order.write(out)?;
    out.bool(t.show_error)
}

/// The keys of a shard's buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keys {
    /// `StringTerms`: the terms.
    Text(Vec<String>),
    /// `LongTerms`: the numbers, or the milliseconds of a date.
    Long(Vec<i64>),
}

impl Keys {
    fn len(&self) -> usize {
        match self {
            Self::Text(v) => v.len(),
            Self::Long(v) => v.len(),
        }
    }
}

/// One sub-aggregation's result in a bucket.
#[derive(Debug, Clone, PartialEq)]
pub struct SubAnswer {
    /// The sub-aggregation's name.
    pub name: String,
    /// Its result.
    pub value: AggValue,
    /// Whether its field is a date.
    pub date: bool,
}

/// A shard's `StringTerms` or `LongTerms`.
#[derive(Debug, Clone, PartialEq)]
pub struct TermsAnswer {
    /// The order the buckets are in (`reduceOrder`): by key.
    pub reduce_order: Order,
    /// The order asked for.
    pub order: OrderSpec,
    /// `size`.
    pub size: i32,
    /// `min_doc_count`.
    pub min_doc_count: i64,
    /// `docCountError`.
    pub doc_count_error: i64,
    /// The shard size.
    pub shard_size: i32,
    /// `show_term_doc_count_error`.
    pub show_error: bool,
    /// The documents of the buckets not kept.
    pub other_doc_count: i64,
    /// The buckets' keys.
    pub keys: Keys,
    /// The buckets' document counts, one a key.
    pub counts: Vec<i64>,
    /// The buckets' error bounds, one a key, when `show_error`.
    pub errors: Vec<i64>,
    /// The buckets' sub-aggregation results, one list a key; empty when the
    /// aggregation has no sub-aggregations.
    pub subs: Vec<Vec<SubAnswer>>,
}

impl TermsAnswer {
    /// The answer's writeable name.
    pub fn name(&self) -> &'static str {
        match self.keys {
            Keys::Text(_) => "sterms",
            Keys::Long(_) => "lterms",
        }
    }

    /// `InternalTerms.doWriteTo` and `InternalMappedTerms.writeTermTypeInfoTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, date: bool, out: &mut Output<'_>) -> Result<(), WireError> {
        out.byte(self.reduce_order.id())?;
        self.order.write(out)?;
        out.count(usize::try_from(self.size).unwrap_or(0))?;
        out.vlong(self.min_doc_count)?;
        out.zlong(self.doc_count_error)?;
        write_format(out, date)?;
        out.count(usize::try_from(self.shard_size).unwrap_or(0))?;
        out.bool(self.show_error)?;
        out.vlong(self.other_doc_count)?;
        out.count(self.keys.len())?;
        for (i, count) in self.counts.iter().enumerate() {
            out.vlong(*count)?;
            if self.show_error {
                out.bytes(&self.errors.get(i).copied().unwrap_or(0).to_be_bytes())?;
            }
            let subs = self.subs.get(i).map_or(&[][..], Vec::as_slice);
            out.count(subs.len())?;
            for sub in subs {
                write_one(&sub.name, &sub.value, sub.date, out)?;
            }
            match &self.keys {
                Keys::Text(terms) => {
                    out.bytes_reference(terms.get(i).map_or(&[][..], |t| t.as_bytes()))?;
                }
                Keys::Long(keys) => out.bytes(&keys.get(i).copied().unwrap_or(0).to_be_bytes())?,
            }
        }
        Ok(())
    }
}
