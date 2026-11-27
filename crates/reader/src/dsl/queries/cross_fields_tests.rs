//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use lucene_analysis::Analyzer;
use lucene_search::extended_query::{BlendedRewrite, ExtendedQuery};
use lucene_search::{BooleanQuery, Clause};

use crate::dsl::parse_chunks;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, to_lucene};
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"body":{"type":"text"},
    "tag":{"type":"keyword"},"kind":{"type":"keyword"},"year":{"type":"integer"}}}"#;

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

type Terms = Vec<(String, String, f32)>;

/// The blended terms of a clause, with the rewrite's tie breaker.
fn blended(c: &Clause) -> (Terms, f32) {
    let Clause::Extended(e) = c else {
        panic!("not extended: {c:?}")
    };
    let ExtendedQuery::Blended(b) = &**e else {
        panic!("not blended: {e:?}")
    };
    let BlendedRewrite::DisjunctionMax(tie) = b.rewrite else {
        panic!("not a dismax blend")
    };
    let terms = b
        .terms
        .iter()
        .map(|(f, t, boost)| (f.clone(), String::from_utf8_lossy(t).into_owned(), *boost))
        .collect();
    (terms, tie)
}

fn t(field: &str, term: &str, boost: f32) -> (String, String, f32) {
    (field.to_owned(), term.to_owned(), boost)
}

#[test]
fn fields_of_one_analyser_blend_each_term() {
    let q = built(
        r#"{"multi_match":{"query":"Quick brown","type":"cross_fields","fields":["title^2","body"]}}"#,
    )
    .expect("builds");
    assert_eq!(q.should.len(), 2, "{q:?}");
    assert_eq!(
        blended(&q.should[0]),
        (vec![t("body", "quick", 1.0), t("title", "quick", 2.0)], 0.0)
    );
    assert_eq!(
        blended(&q.should[1]),
        (vec![t("body", "brown", 1.0), t("title", "brown", 2.0)], 0.0)
    );
    let q = built(
        r#"{"multi_match":{"query":"quick brown","type":"cross_fields","operator":"and","tie_breaker":0.3,"fields":["title","body"]}}"#,
    )
    .expect("builds");
    assert_eq!(q.must.len(), 2, "{q:?}");
    assert_eq!(blended(&q.must[0]).1, 0.3);
}

#[test]
fn a_group_of_one_field_is_a_match_and_groups_combine_by_dismax() {
    // `title` and `body` are one group, `tag` and `kind` another.
    let q = built(
        r#"{"multi_match":{"query":"quick","type":"cross_fields","fields":["title","body","tag","kind"]}}"#,
    )
    .expect("builds");
    let Some(Clause::DisjunctionMax(d)) = q.must.first() else {
        panic!("{q:?}")
    };
    assert_eq!(d.disjuncts.len(), 2, "{d:?}");
    // One field alone is as `match` builds it.
    let q = built(r#"{"multi_match":{"query":"quick","type":"cross_fields","fields":["title"]}}"#)
        .expect("builds");
    assert!(matches!(q.must.first(), Some(Clause::Term(_))), "{q:?}");
}

#[test]
fn a_keyword_group_blends_the_whole_text() {
    let q =
        built(r#"{"multi_match":{"query":"a b","type":"cross_fields","fields":["tag","kind"]}}"#)
            .expect("builds");
    // Keyword terms are constant-scored term queries, not plain terms, so
    // they are beside one another rather than blended.
    assert!(
        matches!(q.must.first(), Some(Clause::DisjunctionMax(_))),
        "{q:?}"
    );
}
