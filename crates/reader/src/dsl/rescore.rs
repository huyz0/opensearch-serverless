//! `rescore`: `RescorerBuilder.parseFromXContent` and
//! `QueryRescorerBuilder.fromXContent` at OpenSearch 3.10.0.

use lucene_search::rescorer::QueryRescoreMode;

use crate::dsl::error::DslError;
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{float_value, int_value};

/// `RescorerBuilder.DEFAULT_WINDOW_SIZE`.
pub const DEFAULT_WINDOW: usize = 10;

/// One query rescorer.
#[derive(Clone, Debug, PartialEq)]
pub struct Rescorer {
    /// How many of the first pass's best hits it rescores.
    pub window: usize,
    /// The query scored over them.
    pub query: Query,
    /// What the first pass's score is multiplied by.
    pub query_weight: f32,
    /// What the rescore query's score is multiplied by.
    pub rescore_query_weight: f32,
    /// How the two join.
    pub mode: QueryRescoreMode,
}

/// The `rescore` array, the parser on its start.
///
/// # Errors
///
/// A [`DslError`] for what stock refuses, and for a window below 1.
pub fn parse_array(p: &mut Parser<'_>) -> Result<Vec<Rescorer>, DslError> {
    // ALLOC: one rescorer an element, bounded by the body's recording.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(out),
            Some(Kind::StartObject) => out.push(parse_one(p)?),
            Some(other) => return Err(unexpected(p, other, "rescore")),
        }
    }
}

/// The `rescore` object, the parser on its start.
///
/// # Errors
///
/// As [`parse_array`].
pub fn parse_object(p: &mut Parser<'_>) -> Result<Vec<Rescorer>, DslError> {
    // ALLOC: one rescorer.
    Ok(vec![parse_one(p)?])
}

fn unexpected(p: &Parser<'_>, kind: Kind, after: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("unexpected token [{}] after [{after}]", kind.name()),
    )
}

/// One rescorer's object: the parser is past its start.
fn parse_one(p: &mut Parser<'_>) -> Result<Rescorer, DslError> {
    let mut window = None;
    let mut found = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() && current == "window_size" => {
                window = Some(int_value(p)?);
            }
            Some(k) if k.is_value() => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("rescore doesn't support [{current}]"),
                ));
            }
            Some(Kind::StartObject) if current == "query" => found = Some(parse_query(p)?),
            Some(Kind::StartObject) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("Unknown key for a START_OBJECT in [{current}]."),
                ));
            }
            Some(other) => return Err(unexpected(p, other, current)),
        }
    }
    let Some(mut rescorer) = found else {
        return Err(DslError::parsing(p.location(), "missing rescore type"));
    };
    if let Some(n) = window {
        rescorer.window = usize::try_from(n)
            .ok()
            .filter(|w| *w > 0)
            .ok_or_else(|| DslError::unsupported("a [rescore] [window_size] below 1"))?;
    }
    Ok(rescorer)
}

/// `QueryRescorerBuilder.fromXContent`: the parser is on the object's start.
fn parse_query(p: &mut Parser<'_>) -> Result<Rescorer, DslError> {
    let mut query = None;
    let mut out = Rescorer {
        window: DEFAULT_WINDOW,
        query: Query::MatchAll { boost: 1.0 },
        query_weight: 1.0,
        rescore_query_weight: 1.0,
        mode: QueryRescoreMode::Total,
    };
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "rescore_query" => {
                query = Some(parse_inner(p)?);
            }
            Some(k) if k.is_value() => match current {
                "query_weight" => out.query_weight = float_value(p)?,
                "rescore_query_weight" => out.rescore_query_weight = float_value(p)?,
                "score_mode" => out.mode = mode(p.text())?,
                _ => return Err(unknown_field(p, current)),
            },
            Some(_) => return Err(unknown_field(p, current)),
        }
    }
    out.query = query.ok_or_else(|| DslError::illegal("rescore_query cannot be null"))?;
    Ok(out)
}

fn unknown_field(p: &Parser<'_>, name: &str) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!("[query] unknown field [{name}]"),
        None,
    )
}

/// `QueryRescoreMode.fromString`.
fn mode(text: &str) -> Result<QueryRescoreMode, DslError> {
    Ok(match text.to_ascii_lowercase().as_str() {
        "avg" => QueryRescoreMode::Avg,
        "max" => QueryRescoreMode::Max,
        "min" => QueryRescoreMode::Min,
        "total" => QueryRescoreMode::Total,
        "multiply" => QueryRescoreMode::Multiply,
        // ALLOC: error path; one message.
        _ => return Err(DslError::illegal(format!("illegal score_mode [{text}]"))),
    })
}
