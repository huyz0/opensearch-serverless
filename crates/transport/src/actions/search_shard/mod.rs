//! The forwarded shard search: `ForwardedSearchRequest` as the Java shell
//! writes it, read for the part of `SearchSourceBuilder` a Rust node can run.
//!
//! A search a Java reader refused as behind the shard's log goes to the
//! shard's owner, which holds the writes that are acknowledged and not yet
//! published. Java writes no length around a field of the source or a query,
//! so the request is read in field order and refused at the first item this
//! node cannot run; nothing after it is read.

mod query;
mod response;
mod write;

pub use query::{Limits, Node, Query, Value};
pub use response::{Hit, MAX_BYTES, Response};

use crate::error::WireError;
use crate::stream::{Input, TaskId};

/// `internal:serverless/search/shard`.
pub const SEARCH_ACTION: &str = "internal:serverless/search/shard";

/// Why a request was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The bytes are not a request.
    Wire(WireError),
    /// A field of the source, or a part of a query, this node does not run.
    Unsupported(&'static str),
    /// A query this node does not run, by its name.
    UnsupportedQuery(String),
    /// A limit passed.
    TooLarge(&'static str),
}

impl From<WireError> for Refusal {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

/// The part of `SearchSourceBuilder` a Rust node runs. Every other field is
/// at its default or the request is refused.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// The query, or none for match-all.
    pub query: Option<Node>,
    /// `from`, or -1 when the request did not set it.
    pub from: i32,
    /// `size`, or -1 when the request did not set it.
    pub size: i32,
    /// `min_score`.
    pub min_score: Option<f32>,
    /// `terminate_after`, 0 for none.
    pub terminate_after: i32,
    /// `timeout` in milliseconds.
    pub timeout_millis: Option<i64>,
    /// `track_total_hits` as a count, when the request set one.
    pub track_total_hits_up_to: Option<i32>,
    /// `_source` as `(fetch, includes, excludes)`, when the request set it.
    pub fetch_source: Option<(bool, Vec<String>, Vec<String>)>,
}

/// One shard's search, forwarded to its owner (`ForwardedSearchRequest`).
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The parent task.
    pub parent: TaskId,
    /// The index name.
    pub index: String,
    /// The shard.
    pub shard: i32,
    /// The part of the source this node runs.
    pub source: Source,
    /// The incarnation the coordinator resolved, or `None` from an old one.
    pub index_uuid: Option<String>,
    /// The instant `now` means for the whole search, in epoch milliseconds.
    pub now_millis: i64,
}

/// How much a request may hold.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// The most bytes of a request.
    pub bytes: usize,
    /// The query limits.
    pub query: Limits,
    /// The most `from` plus `size`.
    pub window: i64,
}

impl Bounds {
    /// The limits of the ADR: 1 MiB, nesting 16, 1,024 queries, a window of
    /// 10,000.
    pub const DEFAULT: Self = Self {
        bytes: 1 << 20,
        query: Limits {
            depth: 16,
            clauses: 1_024,
        },
        window: 10_000,
    };
}

fn none(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// An optional boolean (`writeOptionalBoolean`): 0, 1, or 2 for none. True is
/// refused for `what`.
fn not_true(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    match input.byte(what)? {
        0 | 2 => Ok(()),
        1 => Err(Refusal::Unsupported(what)),
        _ => Err(WireError::malformed(what, "an optional boolean is 0, 1 or 2").into()),
    }
}

fn source(input: &mut Input<'_>, bounds: &Bounds) -> Result<Source, Refusal> {
    none(input, "aggregations")?;
    not_true(input, "explain")?;
    let fetch_source = if input.bool("_source")? {
        let fetch = input.bool("_source fetch")?;
        let includes = input.string_array("_source includes")?;
        Some((fetch, includes, input.string_array("_source excludes")?))
    } else {
        None
    };
    none(input, "docvalue_fields")?;
    none(input, "stored_fields")?;
    let from = input.vint("from")?;
    none(input, "highlight")?;
    if input.vint("indices_boost")? != 0 {
        return Err(Refusal::Unsupported("indices_boost"));
    }
    let min_score = if input.bool("min_score")? {
        Some(query::read_float(input, "min_score")?)
    } else {
        None
    };
    none(input, "post_filter")?;
    let mut budget = query::Budget::new(bounds.query);
    let query = if input.bool("query")? {
        Some(query::node(input, &mut budget)?)
    } else {
        None
    };
    none(input, "rescore")?;
    none(input, "script_fields")?;
    let size = input.vint("size")?;
    none(input, "sort")?;
    none(input, "stats")?;
    none(input, "suggest")?;
    let terminate_after = input.vint("terminate_after")?;
    let timeout_millis = if input.bool("timeout")? {
        Some(time_value(input)?)
    } else {
        None
    };
    none(input, "track_scores")?;
    not_true(input, "version")?;
    not_true(input, "seq_no_primary_term")?;
    if input.vint("ext")? != 0 {
        return Err(Refusal::Unsupported("ext"));
    }
    none(input, "profile")?;
    none(input, "search_after")?;
    none(input, "slice")?;
    none(input, "collapse")?;
    let track_total_hits_up_to = if input.bool("track_total_hits")? {
        Some(input.int("track_total_hits")?)
    } else {
        None
    };
    none(input, "fields")?;
    none(input, "pit")?;
    none(input, "search_pipeline source")?;
    not_true(input, "include_named_queries_score")?;
    none(input, "derived fields object")?;
    none(input, "derived fields")?;
    if input.bool("search_pipeline")? {
        return Err(Refusal::Unsupported("search_pipeline"));
    }
    none(input, "verbose_pipeline")?;
    let window = i64::from(from.max(0)).saturating_add(i64::from(size.max(0)));
    if window > bounds.window {
        return Err(Refusal::TooLarge("from plus size"));
    }
    Ok(Source {
        query,
        from,
        size,
        min_score,
        terminate_after,
        timeout_millis,
        track_total_hits_up_to,
        fetch_source,
    })
}

/// `TimeValue`: a zigzag count and a `TimeUnit` ordinal, as milliseconds.
fn time_value(input: &mut Input<'_>) -> Result<i64, Refusal> {
    let count = input.zlong("timeout")?;
    let nanos_per: i128 = match input.byte("timeout unit")? {
        0 => 1,
        1 => 1_000,
        2 => 1_000_000,
        3 => 1_000_000_000,
        4 => 60 * 1_000_000_000,
        5 => 3_600 * 1_000_000_000,
        6 => 86_400 * 1_000_000_000,
        _ => return Err(WireError::malformed("timeout unit", "a TimeUnit ordinal").into()),
    };
    let millis = i128::from(count).saturating_mul(nanos_per) / 1_000_000;
    i64::try_from(millis).map_err(|_| Refusal::TooLarge("timeout"))
}

impl Request {
    /// Reads a request.
    ///
    /// # Errors
    ///
    /// [`Refusal`] for a request past `bounds`, one that holds a field or a
    /// query this node does not run (named, and nothing after it read), or
    /// bytes that are not a request.
    pub fn read(bytes: &[u8], bounds: &Bounds) -> Result<Self, Refusal> {
        if bytes.len() > bounds.bytes {
            return Err(Refusal::TooLarge("request bytes"));
        }
        let mut input = Input::new(bytes);
        let parent = TaskId::read(&mut input)?;
        let index = input.string("an index")?;
        let shard = input.vint("a shard")?;
        let source = source(&mut input, bounds)?;
        let index_uuid = input.optional_string("an index uuid")?;
        let now_millis = input.long("now")?;
        input.finish("a forwarded search request")?;
        Ok(Self {
            parent,
            index,
            shard,
            source,
            index_uuid,
            now_millis,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn millis(count: i64, unit: u8) -> Result<i64, Refusal> {
        let mut zigzag = u64::from_ne_bytes(count.to_ne_bytes()) << 1;
        if count < 0 {
            zigzag = !zigzag;
        }
        let mut bytes = Vec::new();
        loop {
            let low = u8::try_from(zigzag & 0x7f).unwrap();
            zigzag >>= 7;
            if zigzag == 0 {
                bytes.push(low);
                break;
            }
            bytes.push(low | 0x80);
        }
        bytes.push(unit);
        time_value(&mut Input::new(&bytes))
    }

    #[test]
    fn a_timeout_unit_scales_to_milliseconds() {
        assert_eq!(millis(5_000_000, 0).unwrap(), 5);
        assert_eq!(millis(5_000, 1).unwrap(), 5);
        assert_eq!(millis(5, 2).unwrap(), 5);
        assert_eq!(millis(5, 3).unwrap(), 5_000);
        assert_eq!(millis(5, 4).unwrap(), 300_000);
        assert_eq!(millis(5, 5).unwrap(), 18_000_000);
        assert_eq!(millis(5, 6).unwrap(), 432_000_000);
    }

    #[test]
    fn a_bad_unit_or_an_oversized_timeout_is_refused() {
        assert!(millis(5, 7).is_err());
        assert!(matches!(
            millis(i64::MAX / 2, 6),
            Err(Refusal::TooLarge("timeout"))
        ));
    }

    #[test]
    fn an_optional_boolean_refuses_only_true() {
        for (byte, refused) in [(0_u8, false), (1, true), (2, false)] {
            let got = not_true(&mut Input::new(&[byte]), "explain");
            assert_eq!(matches!(got, Err(Refusal::Unsupported("explain"))), refused);
            assert_eq!(got.is_ok(), !refused);
        }
        assert!(not_true(&mut Input::new(&[3]), "explain").is_err());
    }
}
