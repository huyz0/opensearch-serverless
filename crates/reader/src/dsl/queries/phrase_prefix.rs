//! `match_phrase_prefix`: `MatchPhrasePrefixQueryBuilder.fromXContent` and
//! `doToQuery` with `MatchQuery.parse(PHRASE_PREFIX)` and
//! `TextFieldMapper.createPhrasePrefixQuery` at OpenSearch 3.10.0: the
//! text's positions are whole terms but the last, whose prefix expands over
//! the terms the reader holds.

use lucene_search::Clause;
use lucene_search::extended_query::{ExtendedQuery, MultiPhrasePrefixQuery};
use lucene_search::query_visitor::Occur;

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::queries::full_text::zero_terms;
use crate::dsl::queries::single_field::{self, does_not_support};
use crate::dsl::tokens::Parser;
use crate::dsl::values::{Value, float_value, int_value, object_value};
use crate::failure::Failure;
use crate::lucene_query::{
    QueryContext, field_type, match_all, match_none, text_query, unsupported,
};
use crate::mapping::FieldType;

/// `FuzzyQuery.defaultMaxExpansions`, the query's default `max_expansions`.
const DEFAULT_MAX_EXPANSIONS: i32 = 50;

/// The parameters of a `match_phrase_prefix` query.
#[derive(Clone, Debug, PartialEq)]
pub struct PhrasePrefix {
    /// The field.
    pub field: String,
    /// The text.
    pub query: Value,
    /// How many positions the terms may move.
    pub slop: u32,
    /// How many terms the last prefix expands to, at most.
    pub max_expansions: i32,
    /// `zero_terms_query: all`.
    pub zero_terms_all: bool,
}

fn refused(param: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[match_phrase_prefix] [{param}]"))
}

/// `MatchPhrasePrefixQueryBuilder.fromXContent`.
pub fn parse(p: &mut Parser<'_>) -> Result<More, DslError> {
    let mut value: Option<Value> = None;
    let mut boost = 1.0;
    let mut slop = 0;
    let mut max_expansions = DEFAULT_MAX_EXPANSIONS;
    let mut zero_terms_all = false;
    let field = single_field::parse(p, "match_phrase_prefix", |p, param| {
        match param {
            None | Some("query") => value = object_value(p)?,
            Some("boost") => boost = float_value(p)?,
            Some("slop") => slop = int_value(p)?,
            Some("max_expansions") => max_expansions = int_value(p)?,
            Some("zero_terms_query") => zero_terms_all = zero_terms(p)?,
            Some("analyzer") if p.text() == "standard" => {}
            Some("_name") => crate::dsl::values::name_value(p)?,
            Some(param @ "analyzer") => return Err(refused(param)),
            Some(param) => return Err(does_not_support(p, "match_phrase_prefix", param)),
        }
        Ok(())
    })?;
    let Some(field) = field.filter(|f| !f.is_empty()) else {
        return Err(DslError::illegal(
            "[match_phrase_prefix] requires fieldName",
        ));
    };
    let Some(query) = value else {
        return Err(DslError::illegal(
            "[match_phrase_prefix] requires query value",
        ));
    };
    let Ok(slop) = u32::try_from(slop) else {
        return Err(DslError::illegal("No negative slop allowed."));
    };
    if max_expansions < 0 {
        return Err(DslError::illegal("No negative maxExpansions allowed."));
    }
    let m = PhrasePrefix {
        // ALLOC: one field name a clause, bounded by the body's recording.
        field: field.to_owned(),
        query,
        slop,
        max_expansions,
        zero_terms_all,
    };
    // ALLOC: one box a clause, bounded by the body's recording.
    Ok(More::MatchPhrasePrefix(Box::new(m), boost))
}

/// `MatchQuery.parse(PHRASE_PREFIX, ...)`.
///
/// # Errors
///
/// `illegal_argument_exception` on a field that is not text, and this
/// node's refusal of a text with synonyms.
pub fn build(cx: &QueryContext<'_>, m: &PhrasePrefix) -> Result<Clause, Failure> {
    let field = m.field.as_str();
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    if *ty != FieldType::Text {
        return Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Can only use phrase prefix queries on text fields - not on [{field}] which is of type [{}]",
                super::build::type_name(ty)
            ),
        ));
    }
    let Some(clause) = text_query(cx, field, m.query.text(), Occur::Must, true, m.slop)? else {
        return Ok(if m.zero_terms_all {
            match_all(cx)
        } else {
            match_none()
        });
    };
    // ALLOC: one entry a position, bounded by the analysed text.
    let (terms, positions): (Vec<Vec<u8>>, Vec<i32>) = match clause {
        Clause::Term(t) => (vec![t.term], vec![0]),
        Clause::Phrase(p) => {
            // ALLOC: one entry a position, bounded by the analysed text.
            let positions = if p.positions.is_empty() {
                (0..).take(p.terms.len()).collect()
            } else {
                p.positions
            };
            (p.terms, positions)
        }
        _ => return Err(unsupported("[match_phrase_prefix] of a text with synonyms")),
    };
    // ALLOC: one array a position, bounded by the analysed text.
    let arrays = terms.into_iter().map(|t| vec![t]).collect();
    let q = MultiPhrasePrefixQuery::new(field, arrays, positions, m.slop)
        // ALLOC: error path; one message.
        .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?
        .with_max_expansions(m.max_expansions);
    // ALLOC: one box a phrase prefix query.
    Ok(Clause::Extended(Box::new(
        ExtendedQuery::MultiPhrasePrefix(q),
    )))
}

#[cfg(test)]
#[path = "phrase_prefix_tests.rs"]
mod tests;
