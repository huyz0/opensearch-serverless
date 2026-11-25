//! The query builders of a forwarded search that a Rust node can run, read
//! from the shell's named-writeable encoding.
//!
//! Java writes no length around a query, so a query this module does not
//! know cannot be skipped: it is refused by name and nothing after it is
//! read.

use crate::error::WireError;
use crate::stream::Input;

use super::Refusal;

/// A value Java wrote with `writeGenericValue`, of the kinds a term, terms or
/// range query carries.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `-1`: no value.
    Null,
    /// A string.
    Str(String),
    /// An int.
    Int(i32),
    /// A long.
    Long(i64),
    /// A float.
    Float(f32),
    /// A double.
    Double(f64),
    /// A boolean.
    Bool(bool),
}

/// A query with the boost and name every builder carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The query.
    pub query: Query,
    /// The boost (1.0 by default).
    pub boost: f32,
    /// The `_name`, when set.
    pub name: Option<String>,
}

/// A query this node can run.
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    /// `match_all`.
    MatchAll,
    /// `term`.
    Term {
        /// The field.
        field: String,
        /// The value.
        value: Value,
        /// Whether the match ignores case.
        case_insensitive: bool,
    },
    /// `terms` over an inline list.
    Terms {
        /// The field.
        field: String,
        /// The values.
        values: Vec<Value>,
    },
    /// `range`.
    Range {
        /// The field.
        field: String,
        /// The lower bound.
        from: Value,
        /// The upper bound.
        to: Value,
        /// Whether the lower bound is included.
        include_lower: bool,
        /// Whether the upper bound is included.
        include_upper: bool,
        /// The `time_zone`.
        time_zone: Option<String>,
        /// The `format`.
        format: Option<String>,
        /// The `relation`.
        relation: Option<String>,
    },
    /// `bool`.
    Bool {
        /// `must`.
        must: Vec<Node>,
        /// `must_not`.
        must_not: Vec<Node>,
        /// `should`.
        should: Vec<Node>,
        /// `filter`.
        filter: Vec<Node>,
        /// `adjust_pure_negative`.
        adjust_pure_negative: bool,
        /// `minimum_should_match`.
        minimum_should_match: Option<String>,
    },
    /// `ids`.
    Ids(Vec<String>),
    /// `exists`.
    Exists(String),
}

/// What a query may cost to read: the nesting and the number of queries.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The deepest nesting accepted.
    pub depth: usize,
    /// How many queries one request may hold.
    pub clauses: usize,
}

/// How much of the limits is left while a request is read.
pub(super) struct Budget {
    depth: usize,
    clauses: usize,
}

impl Budget {
    pub(super) fn new(limits: Limits) -> Self {
        Self {
            depth: limits.depth,
            clauses: limits.clauses,
        }
    }
}

fn float(input: &mut Input<'_>, what: &'static str) -> Result<f32, Refusal> {
    let bytes = input.take(4, what)?;
    let mut word = [0_u8; 4];
    word.copy_from_slice(bytes);
    Ok(f32::from_bits(u32::from_be_bytes(word)))
}

fn double(input: &mut Input<'_>, what: &'static str) -> Result<f64, Refusal> {
    let bytes = input.take(8, what)?;
    let mut word = [0_u8; 8];
    word.copy_from_slice(bytes);
    Ok(f64::from_bits(u64::from_be_bytes(word)))
}

pub(super) fn read_float(input: &mut Input<'_>, what: &'static str) -> Result<f32, Refusal> {
    float(input, what)
}

/// `readGenericValue` for the kinds a query holds; any other kind is refused.
fn value(input: &mut Input<'_>) -> Result<Value, Refusal> {
    let tag = input.byte("a value tag")?;
    Ok(match tag {
        0xFF => Value::Null,
        0 => Value::Str(input.string("a value")?),
        1 => Value::Int(input.int("a value")?),
        2 => Value::Long(input.long("a value")?),
        3 => Value::Float(float(input, "a value")?),
        4 => Value::Double(double(input, "a value")?),
        5 => Value::Bool(input.bool("a value")?),
        // A string a builder turned into a `BytesRef`: a length, then UTF-8.
        21 => {
            let raw = input.bytes_reference("a value")?;
            let text = std::str::from_utf8(raw)
                .map_err(|_| WireError::malformed("a value", "a string value is UTF-8"))?;
            Value::Str(text.to_owned()) // ALLOC: one string, at most the bytes of the request
        }
        _ => {
            return Err(Refusal::Unsupported(
                "a value of a kind other than string, number or boolean",
            ));
        }
    })
}

fn values(input: &mut Input<'_>) -> Result<Vec<Value>, Refusal> {
    // 7 is a list; 8 an array of objects, and a set is not a terms value.
    if input.byte("a values tag")? != 7 {
        return Err(Refusal::Unsupported(
            "terms values that are not an inline list",
        ));
    }
    let count = input.count(1, "terms values")?;
    // ALLOC: at most the bytes left, checked by `count`.
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(value(input)?);
    }
    Ok(out)
}

fn clauses(input: &mut Input<'_>, budget: &mut Budget) -> Result<Vec<Node>, Refusal> {
    let count = input.count(1, "bool clauses")?;
    // ALLOC: at most the bytes left, checked by `count`.
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(node(input, budget)?);
    }
    Ok(out)
}

/// `readNamedWriteable` of one query: its name, then its body.
pub(super) fn node(input: &mut Input<'_>, budget: &mut Budget) -> Result<Node, Refusal> {
    if budget.depth == 0 {
        return Err(Refusal::TooLarge("query nesting"));
    }
    if budget.clauses == 0 {
        return Err(Refusal::TooLarge("query clauses"));
    }
    budget.clauses = budget.clauses.saturating_sub(1);
    budget.depth = budget.depth.saturating_sub(1);
    let name = input.string("a query name")?;
    let boost = float(input, "a boost")?;
    let query_name = input.optional_string("a query _name")?;
    let query = match name.as_str() {
        "match_all" => Query::MatchAll,
        "term" => {
            let field = input.string("a field")?;
            let value = value(input)?;
            Query::Term {
                field,
                value,
                case_insensitive: input.bool("case_insensitive")?,
            }
        }
        "terms" => {
            let field = input.string("a field")?;
            if input.bool("terms lookup")? {
                return Err(Refusal::Unsupported("a terms lookup"));
            }
            let values = values(input)?;
            if input.vint("terms value_type")? != 0 {
                return Err(Refusal::Unsupported("terms value_type other than default"));
            }
            Query::Terms { field, values }
        }
        "range" => {
            let field = input.string("a field")?;
            let from = value(input)?;
            let to = value(input)?;
            let include_lower = input.bool("include_lower")?;
            let include_upper = input.bool("include_upper")?;
            let time_zone = if input.bool("time_zone")? {
                Some(input.string("time_zone")?)
            } else {
                None
            };
            Query::Range {
                field,
                from,
                to,
                include_lower,
                include_upper,
                time_zone,
                format: input.optional_string("format")?,
                relation: input.optional_string("relation")?,
            }
        }
        "bool" => {
            let must = clauses(input, budget)?;
            let must_not = clauses(input, budget)?;
            let should = clauses(input, budget)?;
            let filter = clauses(input, budget)?;
            Query::Bool {
                must,
                must_not,
                should,
                filter,
                adjust_pure_negative: input.bool("adjust_pure_negative")?,
                minimum_should_match: input.optional_string("minimum_should_match")?,
            }
        }
        "ids" => Query::Ids(input.string_array("ids")?),
        "exists" => Query::Exists(input.string("a field")?),
        _ => return Err(Refusal::UnsupportedQuery(name)),
    };
    budget.depth = budget.depth.saturating_add(1);
    Ok(Node {
        query,
        boost,
        name: query_name,
    })
}
