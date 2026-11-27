//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use lucene_analysis::Analyzer;
use lucene_search::{BooleanQuery, Clause, PrefixQuery, TermQuery};

use crate::dsl::parse_chunks;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, to_lucene};
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},
    "year":{"type":"integer"}}}"#;

fn parsed(body: &str) -> Result<crate::dsl::SearchSource, Failure> {
    parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).map_err(|e| e.into_failure())
}

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
    let source = parsed(body)?;
    to_lucene(source.query.as_ref().expect("a query"), &cx)
}

fn term(field: &str, text: &str) -> Clause {
    Clause::Term(TermQuery::new(field, text.as_bytes().to_vec()))
}

fn prefix(field: &str, text: &str) -> Clause {
    Clause::Prefix(PrefixQuery::new(field, text.as_bytes().to_vec()))
}

#[test]
fn the_last_term_is_a_prefix_and_the_rest_are_terms() {
    let q = built(r#"{"match_bool_prefix":{"title":"Quick brown fo"}}"#).expect("builds");
    assert_eq!(
        q.should,
        [
            term("title", "quick"),
            term("title", "brown"),
            prefix("title", "fo")
        ]
    );
    let q = built(r#"{"match_bool_prefix":{"title":{"query":"quick fo","operator":"and"}}}"#)
        .expect("builds");
    assert_eq!(q.must, [term("title", "quick"), prefix("title", "fo")]);
}

#[test]
fn one_term_and_other_field_types() {
    let q = built(r#"{"match_bool_prefix":{"title":"Fo"}}"#).expect("builds");
    assert_eq!(q.must, [prefix("title", "fo")]);
    let q = built(r#"{"match_bool_prefix":{"tag":"Ani"}}"#).expect("builds");
    assert_eq!(q.must, [prefix("tag", "Ani")]);
}

#[test]
fn bad_bodies_and_unserved_options_fail() {
    for (body, reason) in [
        (
            r#"{"match_bool_prefix":{}}"#,
            "[match_bool_prefix] requires fieldName",
        ),
        (
            r#"{"match_bool_prefix":{"title":{}}}"#,
            "[match_bool_prefix] requires query value",
        ),
        (
            r#"{"match_bool_prefix":{"title":{"query":"a","x":1}}}"#,
            "[match_bool_prefix] query does not support [x]",
        ),
        (
            r#"{"match_bool_prefix":{"title":"a","tag":"b"}}"#,
            "[match_bool_prefix] query doesn't support multiple fields, found [title] and [tag]",
        ),
    ] {
        assert_eq!(parsed(body).expect_err(body).reason, reason, "{body}");
    }
    for body in [
        r#"{"match_bool_prefix":{"title":{"query":"a","fuzziness":1}}}"#,
        r#"{"match_bool_prefix":{"title":{"query":"a","analyzer":"english"}}}"#,
        r#"{"match_bool_prefix":{"title":{"query":"a","_name":"n"}}}"#,
        r#"{"match_bool_prefix":{"title":{"query":"a","fuzzy_rewrite":"x"}}}"#,
    ] {
        let r = parsed(body).expect_err(body).reason;
        assert!(r.contains("not supported"), "{body}: {r}");
    }
    let r = built(r#"{"match_bool_prefix":{"title":"quick fo."}}"#).expect_err("trailing");
    assert!(r.reason.contains("not supported"), "{r:?}");
}
