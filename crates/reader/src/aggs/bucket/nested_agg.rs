//! `nested` and `reverse_nested`: their bodies
//! (`NestedAggregationBuilder.parse`, `ReverseNestedAggregationBuilder.parse`)
//! and the parent and child queries their engine nodes join over
//! (`NestedAggregatorFactory`, `ReverseNestedAggregatorFactory`).
//!
//! Each is one bucket counted in the engine's fixed result and written as a
//! `filter` is.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_search::{BooleanQuery, Clause};

use crate::aggs::metric::MetricAgg;
use crate::dsl::error::DslError;
use crate::dsl::queries::nested_build::{non_nested, type_filter};
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, match_none};

/// What a `nested` or `reverse_nested` asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NestedSpec {
    /// `reverse_nested`: hands each child's parent on.
    pub reverse: bool,
    /// The nested object's path: required by `nested`, optional for
    /// `reverse_nested` (the root document without one).
    pub path: Option<String>,
}

/// What one bucket of a `nested` holds while collecting: its count and the
/// engine's entry.
const BUCKET_BYTES: usize = 32;

/// The options of a `nested` (or, with `reverse`, a `reverse_nested`) body,
/// the parser on its start.
pub fn parse_nested_body(
    p: &mut Parser<'_>,
    name: &str,
    reverse: bool,
) -> Result<NestedSpec, DslError> {
    let mut spec = NestedSpec {
        reverse,
        path: None,
    };
    let mut key = "";
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                continue;
            }
            Some(kind) => kind,
        };
        if kind == Kind::String && key == "path" {
            // ALLOC: one path, bounded by the body's recording.
            spec.path = Some(p.text().to_owned());
        } else {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!("Unknown key for a {} in [{name}]: [{key}].", kind.name()),
            ));
        }
    }
    if !reverse && spec.path.is_none() {
        return Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("Missing [path] field for nested aggregation [{name}]"),
        ));
    }
    Ok(spec)
}

/// How many queries `agg`'s own node joins over: the parents and children
/// of a `nested`, the parents of a `reverse_nested`.
pub fn own_queries(agg: &MetricAgg) -> usize {
    match agg.nested.as_deref() {
        Some(spec) if spec.reverse => 1,
        Some(_) => 2,
        None => 0,
    }
}

fn refusal(reason: String) -> Failure {
    Failure::opensearch("aggregation_execution_exception", reason)
}

fn filter_of(clause: Clause) -> BooleanQuery {
    BooleanQuery {
        // ALLOC: one clause.
        filter: vec![clause],
        ..BooleanQuery::default()
    }
}

/// Whether `path` names an object (`getObjectMapper`): some field lies under
/// it. A leaf field is no object, so it is unmapped here.
fn is_object(cx: &QueryContext<'_>, path: &str) -> bool {
    cx.mapping
        .names()
        .any(|n| n.strip_prefix(path).is_some_and(|r| r.starts_with('.')))
}

/// A query that matches no document.
fn none() -> BooleanQuery {
    BooleanQuery {
        // ALLOC: one clause.
        must: vec![match_none()],
        ..BooleanQuery::default()
    }
}

/// The queries `agg`'s node joins over, in the engine's order: parents,
/// then (for a `nested`) children. `enclosing` is the path of the `nested`
/// that holds `agg`, if one does.
///
/// # Errors
///
/// A `nested` or `reverse_nested` path that is an object but not nested, or
/// a `reverse_nested` outside a `nested`.
pub fn queries(
    agg: &MetricAgg,
    spec: &NestedSpec,
    cx: &QueryContext<'_>,
    enclosing: Option<&str>,
) -> Result<Vec<BooleanQuery>, Failure> {
    let path = spec.path.as_deref();
    if spec.reverse {
        if enclosing.is_none() {
            // ALLOC: error path; one message.
            return Err(refusal(format!(
                "Reverse nested aggregation [{}] can only be used inside a [nested] aggregation",
                agg.name
            )));
        }
        let parents = match path {
            None => non_nested(),
            Some(p) if cx.mapping.is_nested(p) => filter_of(type_filter(p)),
            // An unmapped path hands on no parent.
            Some(p) if !is_object(cx, p) => none(),
            Some(p) => {
                // ALLOC: error path; one message.
                return Err(refusal(format!(
                    "[reverse_nested] nested path [{p}] is not nested"
                )));
            }
        };
        // ALLOC: one query.
        return Ok(vec![parents]);
    }
    let path = path.unwrap_or_default();
    if !cx.mapping.is_nested(path) {
        if is_object(cx, path) {
            // ALLOC: error path; one message.
            return Err(refusal(format!(
                "[nested] nested path [{path}] is not nested"
            )));
        }
        // Unmapped: no children, so every bucket counts nothing.
        // ALLOC: two queries.
        return Ok(vec![non_nested(), none()]);
    }
    // The parents are the enclosing nested object's documents, else the roots.
    let parents = enclosing.map_or_else(non_nested, |p| filter_of(type_filter(p)));
    // ALLOC: two queries.
    Ok(vec![parents, filter_of(type_filter(path))])
}

/// The engine's node for `agg` over `queries` ([`queries`]'s list).
pub fn node(spec: &NestedSpec, queries: &[BooleanQuery]) -> AggNode {
    let parents = queries.first().cloned().unwrap_or_default();
    if spec.reverse {
        // ALLOC: empty; sub-aggregations are attached after.
        return AggNode::ReverseNested {
            parents,
            subs: Vec::new(),
        };
    }
    AggNode::Nested {
        parents,
        children: queries.get(1).cloned().unwrap_or_default(),
        // ALLOC: empty; sub-aggregations are attached after.
        subs: Vec::new(),
    }
}

/// What a `nested` bucket holds beyond the engine's state.
pub fn state_bytes() -> usize {
    BUCKET_BYTES
}
