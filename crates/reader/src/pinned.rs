//! The tokens a pinned analyzer makes for a query's text
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)): the
//! node supplies a [`PinnedTokens`], and a query builds its terms from a
//! stream over the answer, as it does from a built-in analyzer's.

use lucene_analysis::{AnalysisError, AttributeSource, TokenStream};
use oss_ext::analyzer::Token;

/// What a node gives a query to analyse a pinned field's text.
pub trait PinnedTokens: Send + Sync {
    /// The tokens of `text` under the pin named `analyzer`.
    ///
    /// # Errors
    ///
    /// The words of the refusal; the query fails retryably and never falls
    /// back to the built-in analyzer.
    fn tokens(&self, analyzer: &str, text: &str) -> Result<Vec<Token>, String>;
}

/// Tokens as a stream the query builder reads.
pub struct ListStream {
    atts: AttributeSource,
    tokens: Vec<Token>,
    upto: usize,
}

impl ListStream {
    /// A stream over `tokens`.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            atts: AttributeSource::new(),
            tokens,
            upto: 0,
        }
    }
}

impl TokenStream for ListStream {
    fn conditional_root(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }

    fn attributes(&self) -> &AttributeSource {
        &self.atts
    }

    fn attributes_mut(&mut self) -> &mut AttributeSource {
        &mut self.atts
    }

    fn increment_token(&mut self) -> Result<bool, AnalysisError> {
        let Some(t) = self.tokens.get(self.upto) else {
            return Ok(false);
        };
        self.upto = self.upto.saturating_add(1);
        self.atts.clear_attributes();
        self.atts.set_term(&t.term);
        self.atts.set_offset(
            i32::try_from(t.start).unwrap_or(i32::MAX),
            i32::try_from(t.end).unwrap_or(i32::MAX),
        )?;
        self.atts
            .set_position_increment(i32::try_from(t.increment).unwrap_or(i32::MAX))?;
        Ok(true)
    }

    fn reset(&mut self) -> Result<(), AnalysisError> {
        self.upto = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use lucene_analysis::Analyzer;
    use lucene_search::Clause;

    use super::*;
    use crate::dsl::parse_chunks;
    use crate::failure::Failure;
    use crate::lucene_query::{QueryContext, to_lucene};
    use crate::mapping::Mapping;

    fn token(term: &str, increment: u32) -> Token {
        Token {
            term: term.into(),
            start: 0,
            end: 1,
            increment,
        }
    }

    #[test]
    fn a_stream_gives_each_token_its_term_and_increment_then_ends() {
        let mut s = ListStream::new(vec![token("a", 1), token("b", 3)]);
        s.reset().expect("reset");
        assert!(s.increment_token().expect("first"));
        assert_eq!(s.attributes().term(), "a");
        assert_eq!(s.attributes().position_increment(), 1);
        assert!(s.increment_token().expect("second"));
        assert_eq!(s.attributes().term(), "b");
        assert_eq!(s.attributes().position_increment(), 3);
        assert!(!s.increment_token().expect("end"));
    }

    const MAPPING: &str = r#"{"properties":{"p":{"type":"text","analyzer":"ext_u"},
        "t":{"type":"text"}}}"#;

    /// Splits on `_`, which the standard analyzer does not.
    struct Split;

    impl PinnedTokens for Split {
        fn tokens(&self, analyzer: &str, text: &str) -> Result<Vec<Token>, String> {
            assert_eq!(analyzer, "ext_u");
            Ok(text.split('_').map(|w| token(w, 1)).collect())
        }
    }

    struct Down;

    impl PinnedTokens for Down {
        fn tokens(&self, _: &str, _: &str) -> Result<Vec<Token>, String> {
            Err("module [m] is busy".into())
        }
    }

    fn built(body: &str, pinned: Option<&dyn PinnedTokens>) -> Result<Clause, Failure> {
        let pins = vec!["ext_u".to_owned()];
        let mapping = Mapping::from_json_pinned(MAPPING, &pins).expect("mapping");
        let analyzer = Analyzer::standard(None);
        let cx = QueryContext {
            mapping: &mapping,
            analyzer: &analyzer,
            max_doc: 5,
            knn: None,
            pinned,
        };
        let source = parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).expect("parses");
        let q = to_lucene(source.query.as_ref().expect("a query"), &cx)?;
        Ok(Clause::Boolean(Box::new(q)))
    }

    fn terms(c: &Clause) -> Vec<String> {
        match c {
            Clause::Boolean(b) => b.should.iter().chain(&b.must).flat_map(terms).collect(),
            Clause::Term(t) => vec![String::from_utf8(t.term.clone()).expect("utf8")],
            Clause::Phrase(p) => p
                .terms
                .iter()
                .map(|t| String::from_utf8(t.clone()).expect("utf8"))
                .collect(),
            other => panic!("unexpected {other:?}"),
        }
    }

    // docs/specs/extension-analyzer AC-13
    #[test]
    fn a_match_on_a_pinned_field_takes_its_terms_from_the_pin_not_the_standard_analyzer() {
        let pinned = built(r#"{"match":{"p":"a_b"}}"#, Some(&Split)).expect("builds");
        assert_eq!(terms(&pinned), ["a", "b"]);
        let standard = built(r#"{"match":{"t":"a_b"}}"#, Some(&Split)).expect("builds");
        assert_eq!(terms(&standard), ["a_b"], "an unpinned field is untouched");
    }

    // docs/specs/extension-analyzer AC-13
    #[test]
    fn a_phrase_and_a_multi_match_on_a_pinned_field_use_the_pin() {
        let phrase = built(r#"{"match_phrase":{"p":"a_b"}}"#, Some(&Split)).expect("builds");
        assert_eq!(terms(&phrase), ["a", "b"]);
        let multi = built(
            r#"{"multi_match":{"query":"a_b","fields":["p"],"type":"best_fields"}}"#,
            Some(&Split),
        )
        .expect("builds");
        assert!(terms(&multi).contains(&"a".to_owned()), "{multi:?}");
    }

    // docs/specs/extension-analyzer AC-14
    #[test]
    fn a_node_that_cannot_run_the_pin_or_a_pin_that_fails_refuses_the_query() {
        for pinned in [None, Some(&Down as &dyn PinnedTokens)] {
            let f = built(r#"{"match":{"p":"a_b"}}"#, pinned).expect_err("refused");
            assert_eq!(f.kind, "extension_unavailable", "{f:?}");
        }
        let f = built(r#"{"match":{"p":"a_b"}}"#, Some(&Down)).expect_err("refused");
        assert!(f.reason.contains("busy"), "{}", f.reason);
        // The unpinned field needs no pin.
        built(r#"{"match":{"t":"a_b"}}"#, None).expect("builds");
    }

    // docs/specs/extension-analyzer AC-13
    #[test]
    fn a_prefix_or_fuzzy_term_on_a_pinned_field_is_unsupported_not_standard() {
        let f = built(
            r#"{"query_string":{"query":"a_b*","default_field":"p"}}"#,
            Some(&Split),
        )
        .expect_err("refused");
        assert!(f.reason.contains("pinned analyzer"), "{}", f.reason);
    }
}
