//! `term`, `prefix` and `wildcard` with `case_insensitive: true` as the
//! Lucene queries OpenSearch makes of them: `AutomatonQueries.
//! caseInsensitiveTermQuery`, `caseInsensitivePrefix` and
//! `toCaseInsensitiveWildcardQuery`, each an automaton query over the
//! field's terms under the default constant-score rewrite.

use lucene_search::Clause;
use lucene_search::extended_query::{
    AutomatonQuery, ExtendedQuery, MultiTermQuery, MultiTermSource, RewriteMethod,
};
use lucene_util::automaton::automata::{
    make_any_char, make_any_string, make_case_insensitive_char, make_case_insensitive_string,
};
use lucene_util::automaton::operations::{concatenate, concatenate2, determinize};
use lucene_util::automaton::{Automaton, DEFAULT_DETERMINIZE_WORK_LIMIT, RegExp};

use crate::dsl::queries::build::string_query;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, field_type, match_none, unsupported};
use crate::mapping::FieldType;

/// Which query asked for case-insensitive matching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A `term` query.
    Term,
    /// A `prefix` query.
    Prefix,
    /// A `wildcard` query.
    Wildcard,
    /// A `regexp` query.
    Regexp,
}

/// `toCaseInsensitiveWildcardQuery`'s automaton: `*` any run, `?` one
/// code point, `\` escapes the next one, anything else matches in either case.
fn wildcard_automaton(pattern: &str) -> Automaton {
    // ALLOC: one automaton a code point of the pattern, bounded by the pattern.
    let mut parts: Vec<Automaton> = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        let part = match c {
            '*' => make_any_string(),
            '?' => make_any_char(),
            '\\' => match chars.next() {
                Some(next) => {
                    make_case_insensitive_char(i32::try_from(u32::from(next)).unwrap_or(0))
                }
                // A trailing escape is the character itself.
                None => make_case_insensitive_char(i32::from(b'\\')),
            },
            other => make_case_insensitive_char(i32::try_from(u32::from(other)).unwrap_or(0)),
        };
        parts.push(part);
    }
    // ALLOC: one reference a part, bounded by the pattern.
    let refs: Vec<&Automaton> = parts.iter().collect();
    concatenate(&refs)
}

/// `RegExp(value, ALL, ASCII_CASE_INSENSITIVE)` determinized as a
/// `RegexpQuery` does.
fn regexp_automaton(pattern: &str) -> Result<Automaton, Failure> {
    let refused = |e: String| Failure::plain("illegal_argument_exception", e);
    let regexp = RegExp::with_flags(pattern, RegExp::ALL, RegExp::ASCII_CASE_INSENSITIVE)
        // ALLOC: error path; one message.
        .map_err(|e| refused(e.to_string()))?;
    let nfa = regexp
        .to_automaton()
        // ALLOC: error path; one message.
        .map_err(|e| refused(e.to_string()))?;
    determinize(&nfa, DEFAULT_DETERMINIZE_WORK_LIMIT)
        // ALLOC: error path; one message.
        .map_err(|e| refused(e.to_string()))
}

fn automaton_clause(field: &str, automaton: Automaton) -> Clause {
    // ALLOC: one box an automaton query.
    Clause::Extended(Box::new(ExtendedQuery::MultiTerm(MultiTermQuery::new(
        // ALLOC: the field name, once an automaton query.
        MultiTermSource::Automaton(AutomatonQuery::new(field, automaton, false)),
        RewriteMethod::ConstantScoreBlended,
    ))))
}

/// The query of `kind` for `value` over `field`, matching in either case.
///
/// # Errors
///
/// `query_shard_exception` for a field that is not text or keyword on a
/// prefix or wildcard, and this node's refusal of a `term` on one.
pub fn build(
    cx: &QueryContext<'_>,
    kind: Kind,
    field: &str,
    value: &str,
) -> Result<Clause, Failure> {
    match kind {
        Kind::Term => {
            let Some(ty) = field_type(cx, field)? else {
                return Ok(match_none());
            };
            if !matches!(ty, FieldType::Text | FieldType::Keyword) {
                return Err(unsupported(
                    "[term] [case_insensitive] true on this field type",
                ));
            }
            Ok(automaton_clause(field, make_case_insensitive_string(value)))
        }
        Kind::Prefix => string_query(cx, "prefix", field, || {
            let any = make_any_string();
            automaton_clause(
                field,
                concatenate2(&make_case_insensitive_string(value), &any),
            )
        }),
        Kind::Wildcard => string_query(cx, "wildcard", field, || {
            automaton_clause(field, wildcard_automaton(value))
        }),
        Kind::Regexp => {
            // The length bound, a missing field and a field of another type
            // answer as a plain regexp does.
            let plain = crate::dsl::queries::build::regexp_clause(cx, field, value)?;
            if matches!(plain, Clause::MatchNoDocs(_)) {
                return Ok(plain);
            }
            Ok(automaton_clause(field, regexp_automaton(value)?))
        }
    }
}

#[cfg(test)]
#[path = "case_insensitive_tests.rs"]
mod tests;
