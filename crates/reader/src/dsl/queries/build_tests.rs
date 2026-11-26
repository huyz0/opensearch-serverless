//! Tests of the Lucene queries built; compiled only under `#[cfg(test)]`.

use lucene_analysis::Analyzer;
use lucene_search::extended_query::{ExtendedQuery, PointInSetQuery, PointRangeQuery};
use lucene_search::query::FieldExistsQuery;
use lucene_search::{
    BooleanQuery, BoostQuery, Clause, ConstantScoreQuery, DisjunctionMaxQuery, FuzzyQuery,
    MatchNoDocsQuery, PrefixQuery, RegexpQuery, TermInSetQuery, TermQuery, WildcardQuery,
};

use crate::dsl::bool_query::MAX_CLAUSES;
use crate::dsl::parse_chunks;
use crate::dsl::queries::More;
use crate::dsl::queries::build::{MAX_REGEX_LENGTH, MAX_TERMS_COUNT, to_clause};
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, shard_failure, to_lucene};
use crate::mapping::Mapping;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},
    "year":{"type":"integer"},"n":{"type":"long"},"published":{"type":"date"},
    "geo":{"type":"geo_point"},"hidden":{"type":"keyword","index":false},
    "plain":{"type":"keyword","doc_values":false},
    "obj":{"properties":{"a":{"type":"keyword"},"b":{"type":"long"}}}}}"#;

/// A context over `mapping` with nothing else set.
fn context<'a>(mapping: &'a Mapping, analyzer: &'a Analyzer) -> QueryContext<'a> {
    QueryContext {
        mapping,
        analyzer,
        max_doc: 5,
        knn: None,
        pinned: None,
        now_millis: None,
        shard: None,
    }
}

fn built(body: &str) -> Result<BooleanQuery, Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = context(&mapping, &analyzer);
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

fn none() -> Clause {
    Clause::MatchNoDocs(MatchNoDocsQuery::new())
}

fn constant(c: Clause) -> Clause {
    Clause::ConstantScore(Box::new(ConstantScoreQuery::new(c, 1.0)))
}

fn boosted(c: Clause, boost: f32) -> Clause {
    Clause::Boost(Box::new(BoostQuery::new(c, boost)))
}

fn exists(field: &str) -> Clause {
    constant(Clause::Exists(FieldExistsQuery::new(field)))
}

fn should(clauses: Vec<Clause>) -> Clause {
    Clause::Boolean(Box::new(BooleanQuery {
        should: clauses,
        ..BooleanQuery::default()
    }))
}

#[test]
fn terms_follows_the_field_type() {
    assert_eq!(
        one(r#"{"terms":{"tag":["b","a"]}}"#),
        Clause::TermInSet(TermInSetQuery::new("tag", [b"b".to_vec(), b"a".to_vec()]))
    );
    assert_eq!(
        one(r#"{"terms":{"title":["Quick"],"boost":2}}"#),
        boosted(
            Clause::TermInSet(TermInSetQuery::new("title", [b"Quick".to_vec()])),
            2.0
        )
    );
    assert_eq!(
        one(r#"{"terms":{"year":[2002,"2010",2002.5]}}"#),
        Clause::Extended(Box::new(ExtendedQuery::PointInSet(
            PointInSetQuery::int_set("year", &[2002, 2010]).expect("set")
        )))
    );
    assert_eq!(
        one(r#"{"terms":{"n":[5,"7"]}}"#),
        Clause::Extended(Box::new(ExtendedQuery::PointInSet(
            PointInSetQuery::long_set("n", &[5, 7]).expect("set")
        )))
    );
    let day = |v: &str| {
        crate::lucene_query::term(
            &QueryContext {
                mapping: &Mapping::from_json(MAPPING).expect("mapping"),
                analyzer: &Analyzer::standard(None),
                max_doc: 5,
                knn: None,
                pinned: None,
                now_millis: None,
                shard: None,
            },
            "published",
            &crate::dsl::Value::Str(v.into()),
        )
        .expect("term")
    };
    assert_eq!(
        one(r#"{"terms":{"published":["2001-03-04","1999-12-31"]}}"#),
        constant(should(vec![day("2001-03-04"), day("1999-12-31")]))
    );
}

#[test]
fn terms_that_cannot_match_match_nothing() {
    for body in [
        r#"{"terms":{"tag":[]}}"#,
        r#"{"terms":{"nothing":["a"]}}"#,
        r#"{"terms":{"year":[1.5]}}"#,
        r#"{"terms":{"n":[1.5],"boost":3}}"#,
    ] {
        assert_eq!(one(body), none(), "{body}");
    }
    assert_eq!(
        failure(r#"{"terms":{"year":["x"]}}"#).kind,
        "number_format_exception"
    );
    assert_eq!(
        failure(r#"{"terms":{"n":["x"]}}"#).kind,
        "number_format_exception"
    );
    assert!(
        failure(r#"{"terms":{"geo":["x"]}}"#)
            .reason
            .contains("not supported")
    );
    assert!(
        failure(r#"{"terms":{"hidden":["x"]}}"#)
            .reason
            .contains("not indexed")
    );
}

#[test]
fn string_queries_build_on_string_fields() {
    assert_eq!(
        one(r#"{"prefix":{"tag":"an"}}"#),
        Clause::Prefix(PrefixQuery::new("tag", b"an".to_vec()))
    );
    assert_eq!(
        one(r#"{"wildcard":{"title":{"value":"qu?ck","boost":2}}}"#),
        boosted(
            Clause::Wildcard(WildcardQuery::new("title", b"qu?ck".to_vec())),
            2.0
        )
    );
    assert_eq!(
        one(r#"{"regexp":{"tag":"f.*d"}}"#),
        Clause::Regexp(RegexpQuery::new("tag", "f.*d"))
    );
    assert_eq!(one(r#"{"prefix":{"nothing":"a"}}"#), none());
}

#[test]
fn string_queries_on_other_types_are_shard_exceptions() {
    for (body, kind, ty) in [
        (r#"{"prefix":{"year":"20"}}"#, "prefix", "integer"),
        (r#"{"wildcard":{"n":"2*"}}"#, "wildcard", "long"),
        (r#"{"regexp":{"published":"2.*"}}"#, "regexp", "date"),
    ] {
        let f = failure(body);
        assert_eq!(f.kind, "query_shard_exception");
        assert_eq!(
            f.reason,
            format!(
                "Can only use {kind} queries on keyword and text fields - not on [{}] which is of type [{ty}]",
                body.split('"').nth(3).expect("field")
            )
        );
        let named = shard_failure(f, "i", "u");
        assert_eq!(named.meta.len(), 2, "{named:?}");
    }
}

#[test]
fn fuzzy_builds_its_edits_from_the_term() {
    assert_eq!(
        one(r#"{"fuzzy":{"title":"quikc"}}"#),
        Clause::Fuzzy(FuzzyQuery {
            field: "title".into(),
            term: b"quikc".to_vec(),
            max_edits: 1,
            prefix_length: 0,
            transpositions: true,
            max_expansions: 50,
        })
    );
    assert_eq!(
        one(
            r#"{"fuzzy":{"tag":{"value":"animal","prefix_length":2,"max_expansions":3,"transpositions":false}}}"#
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
    assert_eq!(one(r#"{"fuzzy":{"nothing":"a"}}"#), none());
}

#[test]
fn fuzzy_refuses_other_types_and_bad_bounds() {
    let f = failure(r#"{"fuzzy":{"year":"2001"}}"#);
    assert_eq!(f.kind, "illegal_argument_exception");
    assert_eq!(
        f.reason,
        "Can only use fuzzy queries on keyword and text fields - not on [year] which is of type [integer]"
    );
    assert_eq!(
        failure(r#"{"fuzzy":{"tag":{"value":"a","prefix_length":-1}}}"#).reason,
        "prefixLength cannot be negative."
    );
    assert_eq!(
        failure(r#"{"fuzzy":{"tag":{"value":"a","max_expansions":0}}}"#).reason,
        "maxExpansions must be positive."
    );
}

#[test]
fn exists_finds_fields_by_name_pattern_or_object() {
    assert_eq!(one(r#"{"exists":{"field":"title"}}"#), exists("title"));
    assert_eq!(
        one(r#"{"exists":{"field":"year","boost":2}}"#),
        boosted(exists("year"), 2.0)
    );
    assert_eq!(
        one(r#"{"exists":{"field":"plain"}}"#),
        constant(Clause::Term(TermQuery::new(
            "_field_names",
            b"plain".to_vec()
        )))
    );
    assert_eq!(
        one(r#"{"exists":{"field":"t*"}}"#),
        constant(should(vec![exists("tag"), exists("title")]))
    );
    assert_eq!(
        one(r#"{"exists":{"field":"obj.*"}}"#),
        constant(should(vec![exists("obj.a"), exists("obj.b")]))
    );
    assert_eq!(one(r#"{"exists":{"field":"*.a"}}"#), exists("obj.a"));
    assert_eq!(
        one(r#"{"exists":{"field":"obj"}}"#),
        constant(should(vec![exists("obj.a"), exists("obj.b")]))
    );
    assert_eq!(one(r#"{"exists":{"field":"ob"}}"#), none());
    assert_eq!(one(r#"{"exists":{"field":"z*"}}"#), none());
    assert_eq!(one(r#"{"exists":{"field":"nothing"}}"#), none());
    assert!(
        failure(r#"{"exists":{"field":"geo"}}"#)
            .reason
            .contains("not supported")
    );
}

#[test]
fn simple_match_is_star_globbing() {
    use super::build::simple_match;
    assert!(simple_match("*", "anything"));
    assert!(simple_match("a*c", "abc"));
    assert!(simple_match("a*b*c", "aXbYc"));
    assert!(!simple_match("a*b*c", "aXcYb"));
    assert!(simple_match("abc", "abc"));
    assert!(!simple_match("abc", "abcd"));
    assert!(!simple_match("a*c", "ab"));
    assert!(!simple_match("ab*", "a"));
    assert!(simple_match("*c", "c"));
}

#[test]
fn ids_match_the_encoded_ids() {
    assert_eq!(
        one(r#"{"ids":{"values":["1","doc 1"]}}"#),
        Clause::TermInSet(TermInSetQuery::new(
            "_id",
            [vec![0xfe, 0x1f], b"\xffdoc 1".to_vec()]
        ))
    );
    assert_eq!(one(r#"{"ids":{"values":[]}}"#), none());
    assert_eq!(
        failure(r#"{"ids":{"values":[""]}}"#).reason,
        "Ids can't be empty"
    );
}

#[test]
fn compound_queries_wrap_their_inner_queries() {
    let tag = || {
        Clause::ConstantScore(Box::new(ConstantScoreQuery::new(
            Clause::Term(TermQuery::new("tag", b"a".to_vec())),
            1.0,
        )))
    };
    assert_eq!(
        one(r#"{"constant_score":{"filter":{"term":{"tag":"a"}},"boost":2}}"#),
        boosted(constant(tag()), 2.0)
    );
    assert_eq!(
        one(
            r#"{"dis_max":{"queries":[{"term":{"tag":"a"}},{"match_none":{}}],"tie_breaker":0.3}}"#
        ),
        Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new([tag(), none()], 0.3)))
    );
    assert_eq!(one(r#"{"match_none":{"boost":2}}"#), none());
    assert!(
        failure(r#"{"dis_max":{"queries":[{"prefix":{"year":"1"}}]}}"#)
            .reason
            .starts_with("Can only use prefix")
    );
}

#[test]
fn an_empty_dis_max_matches_nothing() {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = context(&mapping, &analyzer);
    let empty = super::More::DisMax {
        queries: vec![],
        tie_breaker: 0.0,
        boost: 1.0,
    };
    assert_eq!(super::build::to_clause(&empty, &cx), Ok(none()));
}

#[test]
fn point_ranges_stay_beside_point_sets() {
    // A bool of a range and a set keeps both point clauses.
    let q =
        built(r#"{"bool":{"must":[{"range":{"year":{"gte":2000}}},{"terms":{"year":[2001]}}]}}"#)
            .expect("builds");
    assert_eq!(q.must.len(), 2, "{q:?}");
    assert_eq!(
        q.must.first(),
        Some(&Clause::Extended(Box::new(ExtendedQuery::PointRange(
            PointRangeQuery::int_range("year", &[2000], &[i32::MAX]).expect("range")
        ))))
    );
}

fn terms_of(n: usize) -> Result<Clause, Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = context(&mapping, &analyzer);
    // A body this long passes the recorder's bound only if the bound grows,
    // so the query is made directly.
    let m = More::Terms {
        field: "tag".into(),
        values: vec![Value::Str("a".into()); n],
        boost: 1.0,
    };
    to_clause(&m, &cx)
}

#[test]
fn terms_hold_at_most_the_terms_count() {
    assert!(matches!(
        terms_of(MAX_TERMS_COUNT),
        Ok(Clause::TermInSet(_))
    ));
    let Err(f) = terms_of(MAX_TERMS_COUNT + 1) else {
        panic!("built");
    };
    assert_eq!(f.kind, "illegal_argument_exception");
    assert_eq!(
        f.reason,
        "The number of terms [65537] used in the Terms Query request has exceeded the allowed \
         maximum of [65536]. This maximum can be set by changing the [index.max_terms_count] \
         index level setting."
    );
}

#[test]
fn date_terms_hold_at_most_the_clause_limit() {
    let dates = |n: usize| vec!["0"; n].join(",");
    let at = format!(r#"{{"terms":{{"published":[{}]}}}}"#, dates(MAX_CLAUSES));
    assert!(matches!(one(&at), Clause::ConstantScore(_)));
    let over = format!(
        r#"{{"terms":{{"published":[{}]}}}}"#,
        dates(MAX_CLAUSES + 1)
    );
    let f = failure(&over);
    assert_eq!(f.kind, "too_many_clauses");
    assert_eq!(f.reason, "maxClauseCount is set to 1024");
}

#[test]
fn a_regexp_holds_at_most_the_regex_length() {
    let at = format!(
        r#"{{"regexp":{{"tag":"{}"}}}}"#,
        "a".repeat(MAX_REGEX_LENGTH)
    );
    assert!(matches!(one(&at), Clause::Regexp(_)));
    let long = "a".repeat(MAX_REGEX_LENGTH + 1);
    let f = failure(&format!(r#"{{"regexp":{{"tag":"{long}"}}}}"#));
    assert_eq!(f.kind, "illegal_argument_exception");
    assert_eq!(
        f.reason,
        "The length of regex [1001] used in the Regexp Query request has exceeded the allowed \
         maximum of [1000]. This maximum can be set by changing the [index.max_regex_length] \
         index level setting."
    );
    // An unmapped field matches nothing before the length is looked at.
    assert_eq!(
        one(&format!(r#"{{"regexp":{{"nothing":"{long}"}}}}"#)),
        none()
    );
}

#[test]
fn fuzzy_expands_to_at_most_the_clause_limit() {
    let Clause::Fuzzy(q) = one(r#"{"fuzzy":{"tag":{"value":"a","max_expansions":5000}}}"#) else {
        panic!("not fuzzy");
    };
    assert_eq!(q.max_expansions, MAX_CLAUSES);
}

fn built_on_pin(body: &str) -> Result<BooleanQuery, Failure> {
    let mapping = Mapping::from_json_pinned(
        r#"{"properties":{"t":{"type":"text","analyzer":"ext_a"},"title":{"type":"text"}}}"#,
        &["ext_a".to_owned()],
    )
    .expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = context(&mapping, &analyzer);
    let source = parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).expect("parses");
    to_lucene(source.query.as_ref().expect("a query"), &cx)
}

#[test]
fn term_expanding_queries_are_refused_on_a_field_that_pins_an_analyzer() {
    for (kind, body) in [
        ("prefix", r#"{"prefix":{"t":"zz"}}"#),
        ("wildcard", r#"{"wildcard":{"t":"zz*"}}"#),
        ("regexp", r#"{"regexp":{"t":"zz.*"}}"#),
        ("fuzzy", r#"{"fuzzy":{"t":"zzzz"}}"#),
    ] {
        let Err(f) = built_on_pin(body) else {
            panic!("a {kind} query on a pinned field built")
        };
        assert!(
            f.reason.contains(&format!(
                "a [{kind}] query on field [t], which pins the analyzer [ext_a]"
            )),
            "{kind}: {}",
            f.reason
        );
    }
    // The same queries on a field with no pin build as before.
    for body in [
        r#"{"prefix":{"title":"zz"}}"#,
        r#"{"wildcard":{"title":"zz*"}}"#,
        r#"{"regexp":{"title":"zz.*"}}"#,
        r#"{"fuzzy":{"title":"zzzz"}}"#,
    ] {
        assert!(built_on_pin(body).is_ok(), "{body}");
    }
}
