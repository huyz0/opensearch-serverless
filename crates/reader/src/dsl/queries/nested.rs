//! `nested`: `NestedQueryBuilder.fromXContent` at OpenSearch 3.10.0.

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::queries::inner_hits::{InnerHits, parse_inner_hits};
use crate::dsl::queries::single_field::does_not_support;
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value};

/// How the scores of a parent's matching children become its score
/// (`ScoreMode` as `parseScoreMode` names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreMode {
    /// `none`: every parent scores 0.
    None,
    /// `avg`, the default.
    Avg,
    /// `max`.
    Max,
    /// `sum`.
    Sum,
    /// `min`.
    Min,
}

/// A `nested` query.
#[derive(Clone, Debug, PartialEq)]
pub struct NestedQuery {
    /// The nested field's full path.
    pub path: String,
    /// The query over the nested documents.
    pub query: Query,
    /// How child scores combine.
    pub score_mode: ScoreMode,
    /// Whether an unmapped path matches nothing instead of failing.
    pub ignore_unmapped: bool,
    /// `inner_hits`: the matching children to report with each parent.
    pub inner_hits: Option<InnerHits>,
}

/// `NestedQueryBuilder.parseScoreMode`.
fn score_mode(text: &str) -> Result<ScoreMode, DslError> {
    Ok(match text {
        "none" => ScoreMode::None,
        "min" => ScoreMode::Min,
        "max" => ScoreMode::Max,
        "avg" => ScoreMode::Avg,
        "sum" => ScoreMode::Sum,
        // ALLOC: error path; one message.
        other => {
            return Err(DslError::illegal(format!(
                "No score mode for child query [{other}] found"
            )));
        }
    })
}

/// `NestedQueryBuilder.fromXContent`; `_name` is refused.
pub fn parse_nested(p: &mut Parser<'_>) -> Result<More, DslError> {
    let (mut boost, mut mode, mut ignore) = (1.0, ScoreMode::Avg, false);
    let (mut query, mut path) = (None, None);
    let mut inner_hits = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "query" => query = Some(parse_inner(p)?),
            Some(Kind::StartObject) if current == "inner_hits" => {
                inner_hits = Some(parse_inner_hits(p)?);
            }
            Some(Kind::StartObject) => return Err(does_not_support(p, "nested", current)),
            Some(k) if k.is_value() => match current {
                // ALLOC: one path a nested query.
                "path" => path = Some(p.text().to_owned()),
                "boost" => boost = float_value(p)?,
                "ignore_unmapped" => ignore = bool_value(p)?,
                "score_mode" => mode = score_mode(p.text())?,
                "_name" => crate::dsl::values::name_value(p)?,
                _ => return Err(does_not_support(p, "nested", current)),
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("unexpected token [{}]", k.name()),
                ));
            }
        }
    }
    let Some(path) = path else {
        return Err(DslError::illegal("[nested] requires 'path' field"));
    };
    let Some(query) = query else {
        return Err(DslError::illegal("[nested] requires 'query' field"));
    };
    Ok(More::Nested(
        // ALLOC: one box a nested query.
        Box::new(NestedQuery {
            path,
            query,
            score_mode: mode,
            ignore_unmapped: ignore,
            inner_hits,
        }),
        boost,
    ))
}
