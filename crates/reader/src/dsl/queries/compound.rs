//! The compound parsers: `constant_score`, `dis_max`, `boosting` and `match_none`,
//! each its builder's `fromXContent` at OpenSearch 3.10.0.

use crate::dsl::bool_query::MAX_CLAUSES;
use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::queries::single_field::does_not_support;
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::float_value;

/// `ConstantScoreQueryBuilder.fromXContent`.
pub fn parse_constant_score(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut filter = None;
    let mut boost = 1.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "filter" => filter = Some(parse_inner(p)?),
            Some(Kind::StartObject) => return Err(does_not_support(p, "constant_score", current)),
            Some(k) if k.is_value() => match current {
                "boost" => boost = float_value(p)?,
                "_name" => return Err(DslError::unsupported("[constant_score] [_name]")),
                _ => return Err(does_not_support(p, "constant_score", current)),
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
    let Some(filter) = filter else {
        return Err(DslError::parsing(
            p.location(),
            "[constant_score] requires a 'filter' element",
        ));
    };
    Ok(More::ConstantScore { filter, boost })
}

fn push(queries: &mut Vec<Query>, q: Query) -> Result<(), DslError> {
    if queries.len() >= MAX_CLAUSES {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("too many clauses, the maximum is [{MAX_CLAUSES}]"),
        ));
    }
    queries.push(q);
    Ok(())
}

/// `DisMaxQueryBuilder.fromXContent`.
pub fn parse_dis_max(p: &mut Parser<'_>) -> Result<More, DslError> {
    // ALLOC: one entry a disjunct, at most MAX_CLAUSES.
    let mut queries = Vec::new();
    let mut found = false;
    let mut boost = 1.0;
    let mut tie_breaker = 0.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "queries" => {
                found = true;
                push(&mut queries, parse_inner(p)?)?;
            }
            Some(Kind::StartArray) if current == "queries" => {
                found = true;
                // As the Java loops: each element from the array's start or
                // the previous element's end, until the array's end.
                loop {
                    push(&mut queries, parse_inner(p)?)?;
                    if matches!(p.next_token(), Some(Kind::EndArray) | None) {
                        break;
                    }
                }
            }
            Some(Kind::StartObject | Kind::StartArray) => {
                return Err(does_not_support(p, "dis_max", current));
            }
            Some(_) => match current {
                "boost" => boost = float_value(p)?,
                "tie_breaker" => tie_breaker = float_value(p)?,
                "_name" => return Err(DslError::unsupported("[dis_max] [_name]")),
                _ => return Err(does_not_support(p, "dis_max", current)),
            },
        }
    }
    if !found {
        return Err(DslError::parsing(
            p.location(),
            "[dis_max] requires 'queries' field with at least one clause",
        ));
    }
    Ok(More::DisMax {
        queries,
        tie_breaker,
        boost,
    })
}

/// `BoostingQueryBuilder.fromXContent`.
pub fn parse_boosting(p: &mut Parser<'_>) -> Result<More, DslError> {
    let (mut positive, mut negative) = (None, None);
    let (mut boost, mut negative_boost) = (1.0, -1.0);
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) if current == "positive" => positive = Some(parse_inner(p)?),
            Some(Kind::StartObject) if current == "negative" => negative = Some(parse_inner(p)?),
            Some(Kind::StartObject | Kind::StartArray) => {
                return Err(does_not_support(p, "boosting", current));
            }
            Some(_) => match current {
                "negative_boost" => negative_boost = float_value(p)?,
                "boost" => boost = float_value(p)?,
                "_name" => return Err(DslError::unsupported("[boosting] [_name]")),
                _ => return Err(does_not_support(p, "boosting", current)),
            },
        }
    }
    let Some(positive) = positive else {
        return Err(DslError::parsing(
            p.location(),
            "[boosting] query requires 'positive' query to be set'",
        ));
    };
    let Some(negative) = negative else {
        return Err(DslError::parsing(
            p.location(),
            "[boosting] query requires 'negative' query to be set'",
        ));
    };
    if negative_boost < 0.0 {
        return Err(DslError::parsing(
            p.location(),
            "[boosting] query requires 'negative_boost' to be set to be a positive value'",
        ));
    }
    Ok(More::Boosting {
        positive,
        negative,
        negative_boost,
        boost,
    })
}

/// `MatchNoneQueryBuilder.fromXContent`.
pub fn parse_match_none(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut boost = 1.0;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject | Kind::EndArray) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() => match current {
                "boost" => boost = float_value(p)?,
                "_name" => return Err(DslError::unsupported("[match_none] [_name]")),
                _ => return Err(does_not_support(p, "match_none", current)),
            },
            Some(k) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "[match_none] unknown token [{}] after [{current}]",
                        k.name()
                    ),
                ));
            }
        }
    }
    Ok(More::MatchNone { boost })
}
