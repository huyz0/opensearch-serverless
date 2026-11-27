//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use lucene_analysis::Analyzer;
use lucene_search::extended_query::{ExtendedQuery, MultiPhrasePrefixQuery};
use lucene_search::{BooleanQuery, Clause};

use crate::dsl::parse_chunks;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, to_lucene};
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},
    "year":{"type":"integer"}}}"#;

fn built(body: &str) -> Result<BooleanQuery, Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = QueryContext {
        mapping: &mapping,
        analyzer: &analyzer,
        max_doc: 5,
        knn: None,
        pinned: None,
        now_millis: None,
        shard: None,
    };
    let source = parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()])
        .map_err(|e| e.into_failure())?;
    to_lucene(source.query.as_ref().expect("a query"), &cx)
}

fn expected(terms: &[&str], slop: u32, max: i32) -> Clause {
    let arrays = terms.iter().map(|t| vec![t.as_bytes().to_vec()]).collect();
    let positions = (0..terms.len() as i32).collect();
    let q = MultiPhrasePrefixQuery::new("title", arrays, positions, slop)
        .expect("a query")
        .with_max_expansions(max);
    Clause::Extended(Box::new(ExtendedQuery::MultiPhrasePrefix(q)))
}

#[test]
fn terms_become_a_multi_phrase_prefix() {
    let q = built(r#"{"match_phrase_prefix":{"title":"Quick brown fo"}}"#).expect("builds");
    assert_eq!(q.must, [expected(&["quick", "brown", "fo"], 0, 50)]);
    let q = built(
        r#"{"match_phrase_prefix":{"title":{"query":"quick fo","slop":2,"max_expansions":7}}}"#,
    )
    .expect("builds");
    assert_eq!(q.must, [expected(&["quick", "fo"], 2, 7)]);
    let q = built(r#"{"match_phrase_prefix":{"title":"Fo"}}"#).expect("builds");
    assert_eq!(q.must, [expected(&["fo"], 0, 50)]);
}

#[test]
fn other_types_empty_text_and_refusals() {
    let e = built(r#"{"match_phrase_prefix":{"tag":"ani"}}"#).expect_err("keyword");
    assert_eq!(e.kind, "illegal_argument_exception");
    assert!(e.reason.contains("Can only use phrase prefix queries on text fields - not on [tag] which is of type [keyword]"), "{}", e.reason);
    let none = built(r#"{"match_phrase_prefix":{"title":""}}"#).expect("builds");
    assert!(
        matches!(none.must.as_slice(), [Clause::MatchNoDocs(_)]),
        "{none:?}"
    );
    assert!(
        built(r#"{"match_phrase_prefix":{"title":{"query":"a","zero_terms_query":"all"}}}"#)
            .is_ok()
    );
    for body in [
        r#"{"match_phrase_prefix":{"title":{"query":"a","analyzer":"whitespace"}}}"#,
        r#"{"match_phrase_prefix":{"title":{"query":"a","slop":-1}}}"#,
        r#"{"match_phrase_prefix":{"title":{"query":"a","max_expansions":-1}}}"#,
        r#"{"match_phrase_prefix":{"title":{"query":"a","bogus":1}}}"#,
        r#"{"match_phrase_prefix":{"title":{"slop":1}}}"#,
        r#"{"match_phrase_prefix":{"title":"a","tag":"b"}}"#,
    ] {
        assert!(built(body).is_err(), "{body}");
    }
}
