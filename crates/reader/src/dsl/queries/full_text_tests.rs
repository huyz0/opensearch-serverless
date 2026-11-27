//! Tests of `match_phrase` and `multi_match`, parsed and built; compiled
//! only under `#[cfg(test)]`.

use lucene_analysis::Analyzer;
use lucene_search::{BooleanQuery, BoostQuery, Clause, MatchNoDocsQuery, PhraseQuery, TermQuery};

use super::full_text::{MultiMatch, MultiType};
use super::*;
use crate::dsl::parse_chunks;
use crate::dsl::query::Operator;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, to_lucene};
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},
    "year":{"type":"integer"},"geo":{"type":"geo_point"}}}"#;

fn more(body: &str) -> More {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => *m,
        other => panic!("parsed {other:?}"),
    }
}

fn reason(body: &str) -> String {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]) {
        Err(e) => e.into_failure().reason,
        Ok(s) => panic!("parsed {s:?}"),
    }
}

fn built(body: &str) -> Result<Clause, Failure> {
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
    let q: BooleanQuery = to_lucene(source.query.as_ref().expect("a query"), &cx)?;
    assert_eq!(q.must.len(), 1, "{q:?}");
    Ok(q.must.into_iter().next().expect("one"))
}

fn one(body: &str) -> Clause {
    built(body).expect("builds")
}

fn none() -> Clause {
    Clause::MatchNoDocs(MatchNoDocsQuery::new())
}

fn constant(c: Clause) -> Clause {
    Clause::ConstantScore(Box::new(lucene_search::ConstantScoreQuery::new(c, 1.0)))
}

fn phrase_of(c: &Clause) -> &PhraseQuery {
    match c {
        Clause::Phrase(p) => p,
        other => panic!("not a phrase: {other:?}"),
    }
}

fn s(text: &str) -> Value {
    Value::Str(text.into())
}

#[test]
fn match_phrase_reads_short_and_long_forms() {
    assert_eq!(
        more(r#"{"match_phrase":{"title":"brown fox"}}"#),
        More::MatchPhrase {
            field: "title".into(),
            query: s("brown fox"),
            slop: 0,
            zero_terms_all: false,
            boost: 1.0
        }
    );
    assert_eq!(
        more(
            r#"{"match_phrase":{"title":{"query":"a b","slop":2,"zero_terms_query":"ALL","analyzer":"standard","boost":3}}}"#
        ),
        More::MatchPhrase {
            field: "title".into(),
            query: s("a b"),
            slop: 2,
            zero_terms_all: true,
            boost: 3.0
        }
    );
}

#[test]
fn match_phrase_refuses_what_opensearch_refuses() {
    assert_eq!(
        reason(r#"{"match_phrase":{"title":{"query":"a b","slop":-1}}}"#),
        "No negative slop allowed."
    );
    assert_eq!(
        reason(r#"{"match_phrase":{"":"a"}}"#),
        "[match_phrase] requires fieldName"
    );
    assert_eq!(
        reason(r#"{"match_phrase":{"title":{"slop":1}}}"#),
        "[match_phrase] requires query value"
    );
    assert_eq!(
        reason(r#"{"match_phrase":{"title":{"query":"a","zero_terms_query":"some"}}}"#),
        "Unsupported zero_terms_query value [some]"
    );
    assert_eq!(
        reason(r#"{"match_phrase":{"title":{"query":"a","x":1}}}"#),
        "[match_phrase] query does not support [x]"
    );
    let unsupported = r#"{"match_phrase":{"title":{"query":"a","analyzer":"whitespace"}}}"#;
    assert!(
        reason(unsupported).ends_with("is not supported by this node yet"),
        "{unsupported}"
    );
}

#[test]
fn match_phrase_builds_a_phrase_on_text_and_a_term_elsewhere() {
    let c = one(r#"{"match_phrase":{"title":{"query":"Brown Fox","slop":2}}}"#);
    let p = phrase_of(&c);
    assert_eq!(p.field, "title");
    assert_eq!(p.terms, vec![b"brown".to_vec(), b"fox".to_vec()]);
    assert_eq!(p.slop, 2);
    assert_eq!(
        one(r#"{"match_phrase":{"title":"Brown"}}"#),
        Clause::Term(TermQuery::new("title", b"brown".to_vec()))
    );
    assert_eq!(
        one(r#"{"match_phrase":{"tag":"Big Animal"}}"#),
        constant(Clause::Term(TermQuery::new("tag", b"Big Animal".to_vec())))
    );
    assert_eq!(one(r#"{"match_phrase":{"nothing":"a b"}}"#), none());
    assert_eq!(one(r#"{"match_phrase":{"title":"  "}}"#), none());
    assert!(matches!(
        one(r#"{"match_phrase":{"title":{"query":"  ","zero_terms_query":"all"}}}"#),
        Clause::MatchAllDocs(_)
    ));
}

#[test]
fn multi_match_reads_its_parameters() {
    let More::MultiMatch(m, boost) = more(
        r#"{"multi_match":{"query":"a","fields":["title^2","tag","title^3"],"type":"most_fields",
            "operator":"AND","minimum_should_match":"1","tie_breaker":0.5,"slop":1,
            "lenient":true,"zero_terms_query":"all","analyzer":"standard",
            "fuzzy_transpositions":true,"auto_generate_synonyms_phrase_query":false,"boost":4}}"#,
    ) else {
        panic!("not multi_match");
    };
    assert_eq!(boost, 4.0);
    assert_eq!(
        *m,
        MultiMatch {
            query: s("a"),
            fields: vec![
                ("title".into(), 2.0),
                ("tag".into(), 1.0),
                ("title".into(), 3.0)
            ],
            kind: MultiType::MostFields,
            operator: Operator::And,
            minimum_should_match: Some("1".into()),
            tie_breaker: Some(0.5),
            slop: 1,
            zero_terms_all: true,
            lenient: Some(true),
        }
    );
    let More::MultiMatch(m, _) =
        more(r#"{"multi_match":{"query":"a","fields":"tag","type":"boolean"}}"#)
    else {
        panic!("not multi_match");
    };
    assert_eq!(m.fields, vec![("tag".into(), 1.0)]);
    assert_eq!(m.kind, MultiType::BestFields);
    assert_eq!(MultiType::Phrase.tie_breaker(), 0.0);
    assert_eq!(MultiType::MostFields.tie_breaker(), 1.0);
}

#[test]
fn multi_match_refuses_what_opensearch_refuses() {
    assert_eq!(
        reason(r#"{"multi_match":{"fields":["title"]}}"#),
        "No text specified for multi_match query"
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":["title"],"type":"best"}}"#),
        "failed to parse [multi_match] query type [best]. unknown type."
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":["title"],"slop":-1}}"#),
        "No negative slop allowed."
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":["title^x"]}}"#),
        "For input string: \"x\""
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":["title"],"x":1}}"#),
        "[multi_match] query does not support [x]"
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":[{}]}}"#),
        "[multi_match] query does not support [fields]"
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","x":{}}}"#),
        "[multi_match] unknown token [START_OBJECT] after [x]"
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":["title^"]}}"#),
        "empty String"
    );
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":{}}}"#),
        "[multi_match] query does not support [fields]"
    );
    // An array under any key but `fields` is an unknown token.
    assert_eq!(
        reason(r#"{"multi_match":{"query":"a","fields":["title"],"type":["x"]}}"#),
        "[multi_match] unknown token [START_ARRAY] after [type]"
    );
    // The listed-fields bound is four times the clause limit.
    assert_eq!(full_text::MAX_LISTED_FIELDS, 4_096);
    let listed = |n: usize| {
        format!(
            r#"{{"multi_match":{{"query":"a","fields":[{}]}}}}"#,
            vec![r#""title""#; n].join(",")
        )
    };
    assert!(matches!(
        more(&listed(full_text::MAX_LISTED_FIELDS)),
        More::MultiMatch(..)
    ));
    assert!(
        reason(&listed(full_text::MAX_LISTED_FIELDS + 1))
            .ends_with("is not supported by this node yet")
    );
    for unsupported in [
        r#"{"multi_match":{"query":"a"}}"#,
        r#"{"multi_match":{"query":"a","fields":["title"],"type":"phrase_prefix"}}"#,
        r#"{"multi_match":{"query":"a","fields":["title"],"fuzziness":"AUTO"}}"#,
        r#"{"multi_match":{"query":"a","fields":["title"],"analyzer":"simple"}}"#,
    ] {
        assert!(
            reason(unsupported).ends_with("is not supported by this node yet"),
            "{unsupported}"
        );
    }
}

#[test]
fn multi_match_combines_each_field_s_query() {
    let title = || Clause::Term(TermQuery::new("title", b"fox".to_vec()));
    let tag = || constant(Clause::Term(TermQuery::new("tag", b"fox".to_vec())));
    let Clause::DisjunctionMax(d) =
        one(r#"{"multi_match":{"query":"fox","fields":["title^2","tag"]}}"#)
    else {
        panic!("not dis_max");
    };
    assert_eq!(d.tie_breaker, 0.0);
    assert_eq!(
        d.disjuncts,
        vec![
            Clause::Boost(Box::new(BoostQuery::new(title(), 2.0))),
            tag()
        ]
    );
    let Clause::DisjunctionMax(d) =
        one(r#"{"multi_match":{"query":"fox","fields":["title","tag"],"type":"most_fields"}}"#)
    else {
        panic!("not dis_max");
    };
    assert_eq!(d.tie_breaker, 1.0);
    // A name listed again keeps its later boost; patterns multiply.
    assert_eq!(
        one(r#"{"multi_match":{"query":"fox","fields":["title^2","title^5"]}}"#),
        Clause::Boost(Box::new(BoostQuery::new(title(), 5.0)))
    );
    // One field is that field's query; a pattern expands over the mapping.
    assert_eq!(
        one(r#"{"multi_match":{"query":"fox","fields":["title"]}}"#),
        title()
    );
    assert_eq!(
        one(r#"{"multi_match":{"query":"fox","fields":["ti*"]}}"#),
        title()
    );
    assert_eq!(
        one(r#"{"multi_match":{"query":"fox","fields":["title^2","ti*^3"]}}"#),
        Clause::Boost(Box::new(BoostQuery::new(title(), 6.0)))
    );
    let c =
        one(r#"{"multi_match":{"query":"brown fox","fields":["title"],"type":"phrase","slop":1}}"#);
    assert_eq!(phrase_of(&c).slop, 1);
    assert_eq!(
        one(r#"{"multi_match":{"query":"fox","fields":["nothing"]}}"#),
        none()
    );
}

#[test]
fn multi_match_is_lenient_when_asked_or_over_every_field() {
    assert_eq!(
        one(r#"{"multi_match":{"query":"fox","fields":["title","year"],"lenient":true}}"#),
        Clause::DisjunctionMax(Box::new(lucene_search::DisjunctionMaxQuery::new(
            [
                Clause::Term(TermQuery::new("title", b"fox".to_vec())),
                none()
            ],
            0.0
        )))
    );
    let f = built(r#"{"multi_match":{"query":"fox","fields":["title","year"]}}"#)
        .expect_err("not lenient");
    // The shard wraps it as `failed to create query`, which match.json's
    // replay pins.
    assert_eq!(f.kind, "number_format_exception", "{f:?}");
    assert_eq!(f.reason, "For input string: \"fox\"");
    // `*` is lenient by default and leaves out what it cannot search.
    assert!(matches!(
        one(r#"{"multi_match":{"query":"fox","fields":["*"]}}"#),
        Clause::DisjunctionMax(_)
    ));
    // Every field matching nothing leaves the zero-terms answer.
    assert_eq!(
        one(r#"{"multi_match":{"query":"  ","fields":["title"]}}"#),
        none()
    );
}
