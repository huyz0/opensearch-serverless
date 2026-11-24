//! The `knn` query's parser: the k-NN plugin's `KNNQueryBuilder.fromXContent`
//! and its checks, as the stock 3.10.0 node answers them
//! (fixtures/compat/knn/knn-search-*.json).
//!
//! Served: one field with `vector`, `k`, `filter` and `boost`. The radial
//! forms (`min_score`, `max_distance`) and the parameters that change how the
//! graph is walked are refused as unsupported, never ignored.

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{float_value, int_value};
use crate::failure::Failure;

/// The largest `k` (`KNNQueryBuilder.K_MAX`).
pub const MAX_K: usize = 10_000;

/// A parsed `knn` query.
#[derive(Clone, Debug, PartialEq)]
pub struct Knn {
    /// The `knn_vector` field.
    pub field: String,
    /// The query vector.
    pub vector: Vec<f32>,
    /// How many neighbours each shard returns.
    pub k: usize,
    /// The documents the search may return, applied while the graph is
    /// walked.
    pub filter: Option<Query>,
    /// The boost.
    pub boost: f32,
}

/// `[knn] <name> doesn't support values of type: <token>`.
fn wrong_type(p: &Parser<'_>, name: &str, kind: Kind) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!(
            "[knn] {name} doesn't support values of type: {}",
            kind.name()
        ),
        None,
    )
}

/// `[knn] failed to parse field [<name>]`, caused by `cause`.
fn failed(p: &Parser<'_>, name: &str, cause: DslError) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!("[knn] failed to parse field [{name}]"),
        Some(cause),
    )
}

/// A number or numeric string value of parameter `name`.
fn scalar<T>(
    p: &Parser<'_>,
    name: &str,
    read: fn(&Parser<'_>) -> Result<T, DslError>,
) -> Result<T, DslError> {
    match p.current_token() {
        Some(Kind::Number | Kind::String) => read(p).map_err(|e| failed(p, name, e)),
        Some(kind) => Err(wrong_type(p, name, kind)),
        None => Err(DslError::illegal("[knn] unexpected end of input")),
    }
}

/// The `vector` array: the parser is on its start.
fn vector(p: &mut Parser<'_>) -> Result<Vec<f32>, DslError> {
    // ALLOC: one component each of the query vector, bounded by the body's recording.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => break,
            Some(Kind::Number | Kind::String) => {
                out.push(float_value(p).map_err(|e| failed(p, "vector", e))?);
            }
            Some(Kind::StartArray) => {
                return Err(failed(
                    p,
                    "vector",
                    DslError::Failure(Failure::plain(
                        "illegal_state_exception",
                        "expected value but got [START_ARRAY]",
                    )),
                ));
            }
            Some(_) => {
                return Err(DslError::unsupported(
                    "a [knn] vector with a null or object",
                ));
            }
        }
    }
    if out.is_empty() {
        return Err(failed(
            p,
            "vector",
            DslError::illegal("[knn] field 'vector' requires to be non-null and non-empty"),
        ));
    }
    Ok(out)
}

/// What one field's parameters held.
#[derive(Default)]
struct Params {
    vector: Option<Vec<f32>>,
    k: Option<i32>,
    filter: Option<Query>,
    boost: Option<f32>,
    radial: u8,
}

/// The field's parameter object: the parser is on its start.
fn params(p: &mut Parser<'_>) -> Result<Params, DslError> {
    let mut out = Params::default();
    let mut current = "";
    let mut at = p.location();
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(out),
            Some(Kind::FieldName) => {
                current = p.text();
                at = p.location();
            }
            Some(kind) => {
                let seen = match current {
                    "vector" => out.vector.is_some(),
                    "k" => out.k.is_some(),
                    "filter" => out.filter.is_some(),
                    "boost" => out.boost.is_some(),
                    _ => false,
                };
                if seen {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "a repeated [knn] parameter [{current}]"
                    )));
                }
                match (current, kind) {
                    ("vector", Kind::StartArray) => out.vector = Some(vector(p)?),
                    ("vector", _) => {
                        return Err(DslError::unsupported("a [knn] vector that is not an array"));
                    }
                    ("k", _) => out.k = Some(scalar(p, "k", int_value)?),
                    ("boost", _) => out.boost = Some(scalar(p, "boost", float_value)?),
                    ("filter", Kind::StartObject) => {
                        out.filter = Some(parse_inner(p).map_err(|e| failed(p, "filter", e))?);
                    }
                    ("filter", other) => return Err(wrong_type(p, "filter", other)),
                    ("min_score" | "max_distance", _) => {
                        scalar(p, current, float_value)?;
                        out.radial = out.radial.saturating_add(1);
                    }
                    (
                        "method_parameters" | "rescore" | "expand_nested_docs" | "ignore_unmapped"
                        | "_name",
                        _,
                    ) => {
                        // ALLOC: error path; one message.
                        return Err(DslError::unsupported(&format!("[knn] [{current}]")));
                    }
                    _ => {
                        // The field name token is the one before the value.
                        return Err(DslError::xcontent(
                            at,
                            // ALLOC: error path; one message.
                            &format!("[knn] unknown field [{current}]"),
                            None,
                        ));
                    }
                }
            }
        }
    }
}

/// `KNNQueryBuilder.fromXContent` and its constructor's checks.
pub fn parse_knn(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut field: Option<&str> = None;
    let mut found = Params::default();
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                let name = p.text();
                if let Some(first) = field {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "[knn] query doesn't support multiple fields, found [{first}] and [{name}]"
                        ),
                    ));
                }
                field = Some(name);
            }
            Some(Kind::StartObject) => found = params(p)?,
            Some(_) => p.skip_children(),
        }
    }
    let Some(field) = field else {
        return Err(DslError::illegal(
            "[knn] field 'vector' requires to be non-null and non-empty",
        ));
    };
    let counted = u8::from(found.k.is_some()).saturating_add(found.radial);
    if counted != 1 {
        return Err(DslError::illegal(
            "[knn] requires exactly one of k, distance or score to be set",
        ));
    }
    let Some(vector) = found.vector else {
        return Err(DslError::illegal("[knn] requires query vector"));
    };
    let Some(k) = found.k else {
        return Err(DslError::unsupported(
            "a radial [knn] query ([min_score] or [max_distance])",
        ));
    };
    let k = usize::try_from(k)
        .ok()
        .filter(|&k| k > 0 && k <= MAX_K)
        .ok_or_else(|| DslError::illegal("[knn] requires k to be in the range (0, 10000]"))?;
    let boost = found.boost.unwrap_or(1.0);
    if boost < 0.0 {
        return Err(DslError::unsupported("a negative [knn] boost"));
    }
    // ALLOC: one box a knn query, bounded by the body's recording.
    Ok(More::Knn(Box::new(Knn {
        // ALLOC: one field name a query, bounded by the body's recording.
        field: field.to_owned(),
        vector,
        k,
        filter: found.filter,
        boost,
    })))
}
