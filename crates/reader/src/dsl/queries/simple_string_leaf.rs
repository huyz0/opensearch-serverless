//! The leaves `simple_query_string` builds: a word, a phrase, a prefix and
//! a fuzzy term over the resolved fields, as OpenSearch's
//! `SimpleQueryStringQueryParser` builds them.

use lucene_search::{Clause, DisjunctionMaxQuery};

use super::simple_string_build::{Node, Run};
use crate::dsl::queries::Fuzziness;
use crate::dsl::queries::build;
use crate::dsl::queries::full_text::{MultiMatch, MultiType};
use crate::dsl::queries::full_text_build::multi_match;
use crate::dsl::queries::leaf_common::{boosted, no_terms};
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{field_type, match_none};
use crate::mapping::FieldType;

impl Run<'_, '_> {
    /// Whether `text` has no term to search in any of the fields, which
    /// `ZeroTermsQuery.NULL` makes `null`: only text fields drop it.
    pub(super) fn has_no_terms(&self, text: &str, slop: Option<u32>) -> bool {
        no_terms(self.cx, self.fields.iter().map(|(f, _)| *f), text, slop)
    }

    pub(super) fn matched(
        &self,
        kind: MultiType,
        text: &str,
        slop: u32,
    ) -> Result<Option<Node>, Failure> {
        if self.fields.is_empty() {
            return Ok(None);
        }
        let m = MultiMatch {
            // ALLOC: one string a phrase, at most the query.
            query: Value::Str(text.to_owned()),
            // ALLOC: one entry a resolved field, at most the mapping's fields.
            fields: self
                .fields
                .iter()
                // ALLOC: one entry a resolved field, at most the mapping.
                .map(|(f, b)| ((*f).to_owned(), *b))
                // ALLOC: one entry a resolved field, at most the mapping.
                .collect(),
            kind,
            operator: self.s.operator,
            minimum_should_match: None,
            tie_breaker: None,
            slop,
            zero_terms_all: false,
            lenient: Some(self.lenient),
        };
        let clause = multi_match(self.cx, &m)?;
        let none = matches!(clause, Clause::MatchNoDocs(_))
            && self.has_no_terms(text, (kind == MultiType::Phrase).then_some(slop));
        Ok((!none).then_some(Node::Leaf(clause)))
    }

    /// `newDefaultQuery`: `most_fields` over the fields.
    pub(super) fn new_default_query(&self, text: &str) -> Result<Option<Node>, Failure> {
        self.matched(MultiType::MostFields, text, 0)
    }

    /// `newPhraseQuery`: `phrase` over the fields.
    pub(super) fn new_phrase_query(&self, text: &str, slop: i32) -> Result<Option<Node>, Failure> {
        self.matched(
            MultiType::Phrase,
            text,
            u32::try_from(slop.max(0)).unwrap_or(0),
        )
    }

    /// `rethrowUnlessLenient`.
    pub(super) fn lenient_or(&self, failure: Failure) -> Result<Clause, Failure> {
        if self.lenient {
            Ok(match_none())
        } else {
            Err(failure)
        }
    }

    /// `wrapWithBoost`.
    pub(super) fn boosted(query: Clause, boost: f32) -> Clause {
        boosted(query, boost)
    }

    /// The analyzer's `normalize` of a term for a field: a text field's
    /// lowercasing, a keyword field's none.
    pub(super) fn normalized(&self, field: &str, text: &str) -> Result<String, Failure> {
        match field_type(self.cx, field)? {
            Some(FieldType::Text) => {
                let bytes = self
                    .cx
                    .analyzer
                    .normalize(field, text)
                    // ALLOC: error path; one message.
                    .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
            // ALLOC: one string a prefix or fuzzy term, at most the query.
            _ => Ok(text.to_owned()),
        }
    }

    /// The disjuncts of one query a field, combined as `dis_max`.
    pub(super) fn per_field(
        &self,
        mut make: impl FnMut(&str) -> Result<Clause, Failure>,
    ) -> Result<Option<Node>, Failure> {
        if self.fields.is_empty() {
            return Ok(None);
        }
        // ALLOC: one entry a resolved field, at most the mapping's fields.
        let mut disjuncts = Vec::with_capacity(self.fields.len());
        for (field, boost) in &self.fields {
            let query = match make(field) {
                Ok(q) => q,
                Err(e) => self.lenient_or(e)?,
            };
            disjuncts.push(Self::boosted(query, *boost));
        }
        Ok(Some(Node::Leaf(match disjuncts.len() {
            1 => disjuncts.swap_remove(0),
            // ALLOC: one box a dis_max over the fields.
            _ => Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new(disjuncts, 1.0))),
        })))
    }

    /// `newPrefixQuery`, without `analyze_wildcard`.
    pub(super) fn new_prefix_query(&self, text: &str) -> Result<Option<Node>, Failure> {
        self.per_field(|field| {
            let term = self.normalized(field, text)?;
            build::prefix_clause(self.cx, field, &term)
        })
    }

    /// `newFuzzyQuery`.
    pub(super) fn new_fuzzy_query(
        &self,
        text: &str,
        fuzziness: i32,
    ) -> Result<Option<Node>, Failure> {
        self.per_field(|field| {
            let term = self.normalized(field, text)?;
            build::fuzzy(
                self.cx,
                field,
                &Value::Str(term),
                Fuzziness::Edits(u8::try_from(fuzziness).unwrap_or(2)),
                self.s.fuzzy_prefix_length,
                self.s.fuzzy_max_expansions,
                self.s.fuzzy_transpositions,
            )
        })
    }
}
