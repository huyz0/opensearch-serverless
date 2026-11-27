//! The pipeline aggregations of a forwarded search's
//! `AggregatorFactories.Builder`. A shard never runs them (the coordinator
//! does, after the final reduce), so they are read to be passed over, and
//! kept so a request writes back the bytes it read.

use super::Refusal;
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most pipeline aggregations one request may carry.
pub const MAX_PIPELINES: usize = 64;

/// The pipeline aggregations a forwarded search may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineKind {
    /// `avg_bucket`.
    AvgBucket,
    /// `sum_bucket`.
    SumBucket,
    /// `min_bucket`.
    MinBucket,
    /// `max_bucket`.
    MaxBucket,
    /// `stats_bucket`.
    StatsBucket,
    /// `derivative`.
    Derivative,
    /// `cumulative_sum`.
    CumulativeSum,
    /// `bucket_sort`.
    BucketSort,
}

impl PipelineKind {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "avg_bucket" => Self::AvgBucket,
            "sum_bucket" => Self::SumBucket,
            "min_bucket" => Self::MinBucket,
            "max_bucket" => Self::MaxBucket,
            "stats_bucket" => Self::StatsBucket,
            "derivative" => Self::Derivative,
            "cumulative_sum" => Self::CumulativeSum,
            "bucket_sort" => Self::BucketSort,
            _ => return None,
        })
    }

    /// The builder's writeable name.
    pub fn name(self) -> &'static str {
        match self {
            Self::AvgBucket => "avg_bucket",
            Self::SumBucket => "sum_bucket",
            Self::MinBucket => "min_bucket",
            Self::MaxBucket => "max_bucket",
            Self::StatsBucket => "stats_bucket",
            Self::Derivative => "derivative",
            Self::CumulativeSum => "cumulative_sum",
            Self::BucketSort => "bucket_sort",
        }
    }
}

/// One field a `bucket_sort` orders by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortField {
    /// The bucket path it sorts by.
    pub field: String,
    /// `Some(true)` for descending, `Some(false)` for ascending, `None` for
    /// the builder's default.
    pub descending: Option<bool>,
}

/// One pipeline aggregation asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineRequest {
    /// What it computes.
    pub kind: PipelineKind,
    /// Its name.
    pub name: String,
    /// `buckets_path`; a `bucket_sort`'s is its sort fields.
    pub paths: Vec<String>,
    /// The result's format, for the kinds that have one.
    pub format: Option<String>,
    /// The gap policy's id (0 `insert_zeros`, 1 `skip`), for the kinds that
    /// have one; a `derivative` may have none.
    pub gap_policy: Option<u8>,
    /// A `derivative`'s `unit`.
    pub unit: Option<String>,
    /// A `bucket_sort`'s sorts.
    pub sorts: Vec<SortField>,
    /// A `bucket_sort`'s `from`.
    pub from: i32,
    /// A `bucket_sort`'s `size`.
    pub size: Option<i32>,
}

/// A pipeline aggregation and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelinePlace {
    /// The names of the aggregations that hold it, outermost first; none at
    /// the top level.
    pub hosts: Vec<String>,
    /// The pipeline aggregation.
    pub pipeline: PipelineRequest,
}

fn null_map(input: &mut Input<'_>) -> Result<(), Refusal> {
    match input.byte("pipeline aggregation metadata")? {
        0xFF => Ok(()),
        _ => Err(Refusal::Unsupported("pipeline aggregation metadata")),
    }
}

fn absent(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

fn gap(input: &mut Input<'_>) -> Result<u8, Refusal> {
    match input.byte("a gap policy")? {
        g @ (0 | 1) => Ok(g),
        _ => Err(WireError::malformed("a gap policy", "a gap policy is 0 or 1").into()),
    }
}

fn read_sort(input: &mut Input<'_>) -> Result<SortField, Refusal> {
    let field = input.string("a sort field")?;
    absent(input, "a sort's nested filter")?;
    absent(input, "a sort's nested path")?;
    match input.byte("a sort's missing value")? {
        0xFF => {}
        _ => return Err(Refusal::Unsupported("a sort's missing value")),
    }
    let descending = if input.bool("a sort's order")? {
        match input.byte("a sort order")? {
            0 => Some(false),
            1 => Some(true),
            _ => return Err(WireError::malformed("a sort order", "0 or 1").into()),
        }
    } else {
        None
    };
    absent(input, "a sort's mode")?;
    absent(input, "a sort's unmapped type")?;
    absent(input, "a sort's nested sort")?;
    absent(input, "a sort's numeric type")?;
    Ok(SortField { field, descending })
}

/// Reads one pipeline aggregation, after its writeable name.
fn read_one(input: &mut Input<'_>, kind: PipelineKind) -> Result<PipelineRequest, Refusal> {
    let name = input.string("pipeline aggregation name")?;
    let paths = input.string_array("buckets_path")?;
    null_map(input)?;
    let mut p = PipelineRequest {
        kind,
        name,
        paths,
        format: None,
        gap_policy: None,
        unit: None,
        // ALLOC: empty.
        sorts: Vec::new(),
        from: 0,
        size: None,
    };
    match kind {
        PipelineKind::AvgBucket
        | PipelineKind::SumBucket
        | PipelineKind::MinBucket
        | PipelineKind::MaxBucket
        | PipelineKind::StatsBucket => {
            p.format = input.optional_string("a pipeline format")?;
            p.gap_policy = Some(gap(input)?);
        }
        PipelineKind::Derivative => {
            p.format = input.optional_string("a pipeline format")?;
            if input.bool("a derivative's gap policy flag")? {
                p.gap_policy = Some(gap(input)?);
            }
            p.unit = input.optional_string("a derivative's unit")?;
        }
        PipelineKind::CumulativeSum => {
            p.format = input.optional_string("a pipeline format")?;
        }
        PipelineKind::BucketSort => {
            let count = usize::try_from(input.vint("sorts")?)
                .ok()
                .filter(|n| *n <= MAX_PIPELINES)
                .ok_or(Refusal::TooLarge("sorts"))?;
            // ALLOC: one entry a sort, at most 64.
            p.sorts = Vec::with_capacity(count);
            for _ in 0..count {
                p.sorts.push(read_sort(input)?);
            }
            p.from = input.vint("a bucket_sort from")?;
            p.size = if input.bool("a bucket_sort size flag")? {
                Some(input.vint("a bucket_sort size")?)
            } else {
                None
            };
            p.gap_policy = Some(gap(input)?);
        }
    }
    Ok(p)
}

/// Reads the pipeline list of the builder held by `hosts`, appending to
/// `places`; `budget` is how many pipeline aggregations the request may
/// still hold.
pub(super) fn read_list(
    input: &mut Input<'_>,
    hosts: &[String],
    budget: &mut usize,
    places: &mut Vec<PipelinePlace>,
) -> Result<(), Refusal> {
    let count = usize::try_from(input.vint("pipeline aggregations")?)
        .ok()
        .filter(|n| *n <= *budget)
        .ok_or(Refusal::TooLarge("pipeline aggregations"))?;
    *budget = budget.saturating_sub(count);
    for _ in 0..count {
        let type_name = input.string("pipeline aggregation type")?;
        let kind = PipelineKind::from_name(&type_name).ok_or(Refusal::Unsupported(
            "a pipeline aggregation of another type",
        ))?;
        places.push(PipelinePlace {
            // ALLOC: the host names, at most the nesting depth.
            hosts: hosts.to_vec(),
            pipeline: read_one(input, kind)?,
        });
    }
    Ok(())
}

fn write_sort(sort: &SortField, out: &mut Output<'_>) -> Result<(), WireError> {
    out.string(&sort.field)?;
    out.bool(false)?; // nested filter
    out.bool(false)?; // nested path
    out.byte(0xFF)?; // missing: null
    out.bool(sort.descending.is_some())?;
    if let Some(d) = sort.descending {
        out.byte(u8::from(d))?;
    }
    out.bool(false)?; // mode
    out.bool(false)?; // unmapped type
    out.bool(false)?; // nested sort
    out.bool(false) // numeric type
}

fn write_one(p: &PipelineRequest, out: &mut Output<'_>) -> Result<(), WireError> {
    out.string(p.kind.name())?;
    out.string(&p.name)?;
    // ALLOC: one reference a path, at most the request's.
    let paths: Vec<&str> = p.paths.iter().map(String::as_str).collect();
    out.string_array(&paths)?;
    out.byte(0xFF)?; // metadata: null
    let gap = p.gap_policy.unwrap_or(1);
    match p.kind {
        PipelineKind::AvgBucket
        | PipelineKind::SumBucket
        | PipelineKind::MinBucket
        | PipelineKind::MaxBucket
        | PipelineKind::StatsBucket => {
            out.optional_string(p.format.as_deref())?;
            out.byte(gap)
        }
        PipelineKind::Derivative => {
            out.optional_string(p.format.as_deref())?;
            out.bool(p.gap_policy.is_some())?;
            if let Some(g) = p.gap_policy {
                out.byte(g)?;
            }
            out.optional_string(p.unit.as_deref())
        }
        PipelineKind::CumulativeSum => out.optional_string(p.format.as_deref()),
        PipelineKind::BucketSort => {
            out.count(p.sorts.len())?;
            for s in &p.sorts {
                write_sort(s, out)?;
            }
            out.vint(p.from)?;
            out.bool(p.size.is_some())?;
            if let Some(s) = p.size {
                out.vint(s)?;
            }
            out.byte(gap)
        }
    }
}

/// Writes the pipeline list of the builder held by `hosts`.
pub(super) fn write_list(
    places: &[PipelinePlace],
    hosts: &[String],
    out: &mut Output<'_>,
) -> Result<(), WireError> {
    // ALLOC: one reference a pipeline aggregation, at most 64.
    let mine: Vec<&PipelinePlace> = places.iter().filter(|p| p.hosts == hosts).collect();
    out.count(mine.len())?;
    for place in mine {
        write_one(&place.pipeline, out)?;
    }
    Ok(())
}
