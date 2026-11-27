//! `BoolQueryBuilder`'s parser: an `ObjectParser` named `bool`.
//!
//! Ported from `BoolQueryBuilder.PARSER` and the parts of
//! `ObjectParser.parse` it uses (libs/x-content/src/main/java/org/opensearch/core/xcontent/ObjectParser.java)
//! at OpenSearch 3.10.0: an unknown field is an `XContentParseException`
//! at the field's name, with suggestions among the declared names; a value
//! of the wrong type is refused before it is parsed; and a failure inside a
//! field's parser is wrapped as "failed to parse field".

use crate::dsl::error::DslError;
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, name_value, suggest, text_or_null};

/// The fields `BoolQueryBuilder.PARSER` declares, as its candidates.
const FIELDS: &[&str] = &[
    "must",
    "should",
    "must_not",
    "filter",
    "adjust_pure_negative",
    "minimum_should_match",
    "_name",
    "boost",
];

/// The most clauses one `bool` may hold, `indices.query.bool.max_clause_count`'s
/// default (Lucene's `IndexSearcher.getMaxClauseCount`).
pub const MAX_CLAUSES: usize = 1024;

/// A parsed `bool` query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoolQuery {
    /// `must` clauses.
    pub must: Vec<Query>,
    /// `should` clauses.
    pub should: Vec<Query>,
    /// `must_not` clauses.
    pub must_not: Vec<Query>,
    /// `filter` clauses.
    pub filter: Vec<Query>,
    /// `minimum_should_match`.
    pub minimum_should_match: Option<String>,
    /// `adjust_pure_negative`, true by default.
    pub adjust_pure_negative: bool,
    /// The boost.
    pub boost: f32,
}

/// `BoolQueryBuilder.fromXContent`: on the object's start; ends on its end.
pub fn parse_bool(p: &mut Parser<'_>) -> Result<BoolQuery, DslError> {
    let mut q = BoolQuery {
        adjust_pure_negative: true,
        boost: 1.0,
        ..BoolQuery::default()
    };
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(q),
            Some(Kind::FieldName) => {}
            Some(_) => {
                return Err(DslError::xcontent(
                    p.location(),
                    "[bool] no field found",
                    None,
                ));
            }
        }
        let name = p.text();
        let at = p.location();
        if !FIELDS.contains(&name) {
            return Err(DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!("[bool] unknown field [{name}]{}", suggest(name, FIELDS)),
                None,
            ));
        }
        let token = p.next_token();
        let clauses = matches!(name, "must" | "should" | "must_not" | "filter");
        let allowed = match token {
            Some(Kind::StartObject | Kind::StartArray | Kind::Null) => clauses,
            Some(Kind::Number | Kind::String) => !clauses && name != "adjust_pure_negative",
            Some(Kind::Bool) => matches!(name, "adjust_pure_negative" | "minimum_should_match"),
            _ => false,
        };
        if !allowed {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[bool] {name} doesn't support values of type: {}",
                    token.map_or("null", Kind::name)
                ),
                None,
            ));
        }
        field(p, &mut q, name).map_err(|cause| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[bool] failed to parse field [{name}]"),
                Some(cause),
            )
        })?;
    }
}

fn field(p: &mut Parser<'_>, q: &mut BoolQuery, name: &str) -> Result<(), DslError> {
    match name {
        "must" => clauses(p, &mut q.must),
        "should" => clauses(p, &mut q.should),
        "must_not" => clauses(p, &mut q.must_not),
        "filter" => clauses(p, &mut q.filter),
        "adjust_pure_negative" => {
            q.adjust_pure_negative = bool_value(p)?;
            Ok(())
        }
        "minimum_should_match" => {
            // ALLOC: one string a bool, bounded by the body's recording.
            q.minimum_should_match = text_or_null(p).map(str::to_owned);
            Ok(())
        }
        "boost" => {
            q.boost = float_value(p)?;
            Ok(())
        }
        "_name" => name_value(p),
        _ => Err(DslError::unsupported("[bool] [_name]")),
    }
}

/// `declareObjectArrayOrNull`: one object, an array of them, or null.
fn clauses(p: &mut Parser<'_>, into: &mut Vec<Query>) -> Result<(), DslError> {
    match p.current_token() {
        Some(Kind::StartArray) => loop {
            match p.next_token() {
                Some(Kind::EndArray) | None => return Ok(()),
                _ => push(into, parse_inner(p)?)?,
            }
        },
        Some(Kind::Null) => Ok(()),
        _ => push(into, parse_inner(p)?),
    }
}

fn push(into: &mut Vec<Query>, q: Query) -> Result<(), DslError> {
    if into.len() >= MAX_CLAUSES {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("too many clauses, the maximum is [{MAX_CLAUSES}]"),
        ));
    }
    into.push(q);
    Ok(())
}
