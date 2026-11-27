//! Tests of `match` with a `fuzziness`; compiled only under `#[cfg(test)]`.

use lucene_analysis::Analyzer;
use lucene_search::{BooleanQuery, Clause, FuzzyQuery};

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
    let source = parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).expect("parses");
    to_lucene(source.query.as_ref().expect("a query"), &cx)
}

fn one(body: &str) -> Clause {
    let q = built(body).expect("builds");
    assert_eq!(q.must.len(), 1, "{q:?}");
    q.must.into_iter().next().expect("one")
}

fn failure(body: &str) -> Failure {
    match built(body) {
        Err(f) => f,
        Ok(q) => panic!("built {q:?}"),
    }
}

fn fuzzy_clause(field: &str, term: &str, max_edits: u8, prefix_length: usize) -> Clause {
    Clause::Fuzzy(FuzzyQuery {
        field: field.into(),
        term: term.as_bytes().to_vec(),
        max_edits,
        prefix_length,
        transpositions: true,
        max_expansions: 50,
    })
}

#[test]
fn a_fuzzy_match_makes_each_analysed_term_a_fuzzy_query() {
    let q = built(r#"{"match":{"title":{"query":"quikc brown","fuzziness":1}}}"#).expect("builds");
    assert_eq!(
        q.should,
        [
            fuzzy_clause("title", "quikc", 1, 0),
            fuzzy_clause("title", "brown", 1, 0)
        ]
    );
    let q = built(r#"{"match":{"title":{"query":"quikc brown","fuzziness":0,"operator":"and"}}}"#)
        .expect("builds");
    assert_eq!(
        q.must,
        [
            fuzzy_clause("title", "quikc", 0, 0),
            fuzzy_clause("title", "brown", 0, 0)
        ]
    );
    assert_eq!(
        one(r#"{"match":{"title":{"query":"quikc","fuzziness":"AUTO"}}}"#),
        fuzzy_clause("title", "quikc", 1, 0)
    );
}

#[test]
fn a_fuzzy_match_keeps_its_fuzzy_options_and_reaches_keyword_fields() {
    assert_eq!(
        one(
            r#"{"match":{"tag":{"query":"animal","fuzziness":"AUTO","prefix_length":2,"max_expansions":3,"fuzzy_transpositions":false}}}"#
        ),
        Clause::Fuzzy(FuzzyQuery {
            field: "tag".into(),
            term: b"animal".to_vec(),
            max_edits: 2,
            prefix_length: 2,
            transpositions: false,
            max_expansions: 3,
        })
    );
    assert!(
        failure(r#"{"match":{"year":{"query":"3","fuzziness":1}}}"#)
            .reason
            .starts_with("Can only use fuzzy queries")
    );
}

#[test]
fn a_fuzzy_match_without_fuzziness_stays_a_plain_match() {
    let q =
        built(r#"{"match":{"title":{"query":"quikc brown","prefix_length":2}}}"#).expect("builds");
    assert!(
        q.should.iter().all(|c| matches!(c, Clause::Term(_))),
        "{q:?}"
    );
}
