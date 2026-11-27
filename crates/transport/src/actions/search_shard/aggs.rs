//! The aggregations of a forwarded search: `AggregatorFactories.Builder` as
//! the Java shell writes it, for the single-value metrics a Rust node runs
//! (`min`, `max`, `sum`, `avg`, `value_count` over one field), and the
//! shard's `InternalAggregations` answering them.
//!
//! Anything past that (a sub-aggregation, a pipeline aggregation, metadata, a
//! script, a format, a `missing` value, a time zone, another type) is
//! refused by name.

use super::Refusal;
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most aggregations one request may carry.
pub const MAX_AGGREGATIONS: usize = 64;

/// The metric aggregations a forwarded search may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggKind {
    /// `min`.
    Min,
    /// `max`.
    Max,
    /// `sum`.
    Sum,
    /// `avg`.
    Avg,
    /// `value_count`.
    ValueCount,
}

impl AggKind {
    /// The aggregation's type name, which is its writeable name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Max => "max",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::ValueCount => "value_count",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::Min, Self::Max, Self::Sum, Self::Avg, Self::ValueCount]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// One aggregation asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggRequest {
    /// The aggregation's name.
    pub name: String,
    /// What it computes.
    pub kind: AggKind,
    /// The field it reads.
    pub field: String,
}

/// One shard-level result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AggValue {
    /// `InternalMin`'s value; positive infinity when nothing matched.
    Min(f64),
    /// `InternalMax`'s value; negative infinity when nothing matched.
    Max(f64),
    /// `InternalSum`'s value.
    Sum(f64),
    /// `InternalAvg`'s sum and count.
    Avg {
        /// The sum.
        sum: f64,
        /// How many values.
        count: i64,
    },
    /// `InternalValueCount`'s value.
    ValueCount(i64),
}

/// A result with the name of its aggregation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AggAnswer<'a> {
    /// The aggregation's name.
    pub name: &'a str,
    /// Its value.
    pub value: AggValue,
}

/// A generic value that is null (`writeGenericValue(null)` is the byte -1).
fn null_generic(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    match input.byte(what)? {
        0xFF => Ok(()),
        _ => Err(Refusal::Unsupported(what)),
    }
}

fn none(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// An empty `AggregatorFactories.Builder`: no aggregation, no pipeline.
fn empty_builder(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.vint(what)? != 0 || input.vint(what)? != 0 {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// Reads the `AggregatorFactories.Builder` after its presence flag was read
/// as true.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or an aggregation this node does not run.
pub fn read(input: &mut Input<'_>) -> Result<Vec<AggRequest>, Refusal> {
    let count = input.vint("aggregations")?;
    let count = usize::try_from(count)
        .ok()
        .filter(|n| *n <= MAX_AGGREGATIONS)
        .ok_or(Refusal::TooLarge("aggregations"))?;
    // ALLOC: one entry an aggregation, at most MAX_AGGREGATIONS.
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let kind_name = input.string("aggregation type")?;
        let Some(kind) = AggKind::from_name(&kind_name) else {
            return Err(Refusal::Unsupported("an aggregation of another type"));
        };
        let name = input.string("aggregation name")?;
        empty_builder(input, "sub-aggregations")?;
        null_generic(input, "aggregation metadata")?;
        if kind == AggKind::ValueCount {
            // `serializeTargetValueType`: a null writeable.
            none(input, "a value type target")?;
        }
        let field = input
            .optional_string("aggregation field")?
            .ok_or(Refusal::Unsupported("an aggregation without a field"))?;
        none(input, "an aggregation script")?;
        none(input, "an aggregation value type")?;
        if input.optional_string("aggregation format")?.is_some() {
            return Err(Refusal::Unsupported("an aggregation format"));
        }
        null_generic(input, "an aggregation missing value")?;
        none(input, "an aggregation time zone")?;
        out.push(AggRequest { name, kind, field });
    }
    // Pipeline aggregations.
    if input.vint("pipeline aggregations")? != 0 {
        return Err(Refusal::Unsupported("pipeline aggregations"));
    }
    Ok(out)
}

/// Writes `aggs` as `AggregatorFactories.Builder.writeTo` does.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write(aggs: &[AggRequest], out: &mut Output<'_>) -> Result<(), WireError> {
    out.count(aggs.len())?;
    for agg in aggs {
        out.string(agg.kind.name())?;
        out.string(&agg.name)?;
        out.vint(0)?; // sub-aggregations
        out.vint(0)?; // sub pipeline aggregations
        out.byte(0xFF)?; // metadata: null
        if agg.kind == AggKind::ValueCount {
            out.bool(false)?; // target value type
        }
        out.optional_string(Some(&agg.field))?;
        out.bool(false)?; // script
        out.bool(false)?; // value type
        out.optional_string(None)?; // format
        out.byte(0xFF)?; // missing: null
        out.bool(false)?; // time zone
    }
    out.vint(0) // pipeline aggregations
}

/// Writes `answers` as `InternalAggregations.writeTo` does: a list of named
/// writeables, each its type name and its `InternalAggregation.writeTo`.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write_answers(answers: &[AggAnswer<'_>], out: &mut Output<'_>) -> Result<(), WireError> {
    out.count(answers.len())?;
    for a in answers {
        let (kind, ()) = match a.value {
            AggValue::Min(_) => (AggKind::Min, ()),
            AggValue::Max(_) => (AggKind::Max, ()),
            AggValue::Sum(_) => (AggKind::Sum, ()),
            AggValue::Avg { .. } => (AggKind::Avg, ()),
            AggValue::ValueCount(_) => (AggKind::ValueCount, ()),
        };
        out.string(kind.name())?;
        out.string(a.name)?;
        out.byte(0xFF)?; // metadata: null
        match a.value {
            AggValue::Min(v) | AggValue::Max(v) | AggValue::Sum(v) => {
                out.string("raw")?; // DocValueFormat.RAW, which writes nothing more
                out.bytes(&v.to_bits().to_be_bytes())?;
            }
            AggValue::Avg { sum, count } => {
                out.string("raw")?;
                out.bytes(&sum.to_bits().to_be_bytes())?;
                out.vlong(count)?;
            }
            AggValue::ValueCount(n) => out.vlong(n)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(aggs: &[AggRequest]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut out = Output::new(&mut buf, 1 << 20);
        write(aggs, &mut out).expect("writes");
        buf
    }

    #[test]
    fn a_written_request_reads_back() {
        let aggs = vec![
            AggRequest {
                name: "hi".into(),
                kind: AggKind::Max,
                field: "n".into(),
            },
            AggRequest {
                name: "c".into(),
                kind: AggKind::ValueCount,
                field: "tag".into(),
            },
        ];
        let wire = bytes(&aggs);
        let mut input = Input::new(&wire);
        assert_eq!(read(&mut input).expect("reads"), aggs);
        input.finish("aggregations").expect("all read");
    }

    #[test]
    fn another_type_and_a_sub_aggregation_are_refused() {
        let mut wire = Vec::new();
        let mut out = Output::new(&mut wire, 1 << 20);
        out.vint(1).expect("count");
        out.string("terms").expect("type");
        assert!(matches!(
            read(&mut Input::new(&wire)),
            Err(Refusal::Unsupported("an aggregation of another type"))
        ));
        let mut wire = Vec::new();
        let mut out = Output::new(&mut wire, 1 << 20);
        out.vint(1).expect("count");
        out.string("max").expect("type");
        out.string("m").expect("name");
        out.vint(1).expect("a sub-aggregation");
        out.vint(0).expect("no pipelines");
        assert!(matches!(
            read(&mut Input::new(&wire)),
            Err(Refusal::Unsupported("sub-aggregations"))
        ));
    }

    #[test]
    fn a_reply_is_a_list_of_named_writeables() {
        let mut buf = Vec::new();
        let mut out = Output::new(&mut buf, 1 << 20);
        let answers = [
            AggAnswer {
                name: "m",
                value: AggValue::Max(2.0),
            },
            AggAnswer {
                name: "c",
                value: AggValue::ValueCount(7),
            },
        ];
        write_answers(&answers, &mut out).expect("writes");
        let mut expected = vec![2, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w'];
        expected.extend_from_slice(&2.0f64.to_bits().to_be_bytes());
        expected.extend_from_slice(&[11]);
        expected.extend_from_slice(b"value_count");
        expected.extend_from_slice(&[1, b'c', 0xFF, 7]);
        assert_eq!(buf, expected);
    }
}
