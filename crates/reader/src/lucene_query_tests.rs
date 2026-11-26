//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;
use crate::date;
use crate::dsl::parse_chunks;

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"},"tag":{"type":"keyword"},
    "year":{"type":"integer"},"n":{"type":"long"},"published":{"type":"date"},
    "geo":{"type":"geo_point"},"hidden":{"type":"keyword","index":false}}}"#;

fn built(body: &str) -> Result<BooleanQuery, Failure> {
    built_at(body, None)
}

fn built_at(body: &str, now_millis: Option<i64>) -> Result<BooleanQuery, Failure> {
    let mapping = Mapping::from_json(MAPPING).expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = QueryContext {
        mapping: &mapping,
        analyzer: &analyzer,
        max_doc: 5,
        knn: None,
        pinned: None,
        now_millis,
        shard: None,
    };
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    to_lucene(source.query.as_ref().expect("a query"), &cx)
}

fn one(body: &str) -> Clause {
    let q = built(body).expect("builds");
    assert_eq!(q.must.len(), 1, "{q:?}");
    q.must.into_iter().next().expect("one")
}

fn reason(body: &str) -> String {
    match built(body) {
        Err(f) => f.reason,
        Ok(q) => panic!("built {q:?}"),
    }
}

fn term(field: &str, t: &str) -> Clause {
    Clause::Term(TermQuery::new(field, t.as_bytes().to_vec()))
}

fn keyword(field: &str, t: &str) -> Clause {
    Clause::ConstantScore(Box::new(ConstantScoreQuery::new(term(field, t), 1.0)))
}

fn ints(field: &str, l: i32, u: i32) -> Clause {
    Clause::Extended(Box::new(ExtendedQuery::PointRange(
        PointRangeQuery::int_range(field, &[l], &[u]).expect("range"),
    )))
}

fn longs(field: &str, l: i64, u: i64) -> Clause {
    Clause::Extended(Box::new(ExtendedQuery::PointRange(
        PointRangeQuery::long_range(field, &[l], &[u]).expect("range"),
    )))
}

fn none() -> Clause {
    Clause::MatchNoDocs(MatchNoDocsQuery::new())
}

fn all() -> Clause {
    Clause::MatchAllDocs(MatchAllDocsQuery::new(5))
}

#[test]
fn term_queries_follow_the_field_type() {
    assert_eq!(
        one(r#"{"query":{"term":{"title":"Brown"}}}"#),
        term("title", "Brown")
    );
    assert_eq!(
        one(r#"{"query":{"term":{"tag":"animal"}}}"#),
        keyword("tag", "animal")
    );
    assert_eq!(
        one(r#"{"query":{"term":{"year":2002}}}"#),
        ints("year", 2002, 2002)
    );
    assert_eq!(
        one(r#"{"query":{"term":{"year":"2010"}}}"#),
        ints("year", 2010, 2010)
    );
    assert_eq!(one(r#"{"query":{"term":{"year":2002.5}}}"#), none());
    assert_eq!(
        one(r#"{"query":{"term":{"n":9007199254740993}}}"#),
        longs("n", 9_007_199_254_740_993, 9_007_199_254_740_993)
    );
    assert_eq!(one(r#"{"query":{"term":{"n":1.5}}}"#), none());
    assert_eq!(one(r#"{"query":{"term":{"nope":"x"}}}"#), none());
    let day = date::parse_to_millis("2002-07-01", false).expect("date");
    assert_eq!(
        one(r#"{"query":{"term":{"published":"2002-07-01"}}}"#),
        longs("published", day, day + 86_400_000 - 1),
        "a date term is the range of its rounding"
    );
}

#[test]
fn boosts_wrap_unless_one_or_nothing_matches() {
    assert_eq!(
        one(r#"{"query":{"term":{"tag":{"value":"manual","boost":2.0}}}}"#),
        Clause::Boost(Box::new(BoostQuery::new(keyword("tag", "manual"), 2.0)))
    );
    assert_eq!(
        one(r#"{"query":{"term":{"nope":{"value":"x","boost":2.0}}}}"#),
        none()
    );
    assert_eq!(one(r#"{"query":{"match_all":{"boost":1.0}}}"#), all());
}

#[test]
fn match_queries_analyse_text_and_term_everything_else() {
    assert_eq!(
        one(r#"{"query":{"match":{"title":"Fox"}}}"#),
        term("title", "fox")
    );
    let q = built(r#"{"query":{"match":{"title":"Quick Brown"}}}"#).expect("builds");
    assert_eq!(
        q.should,
        vec![term("title", "quick"), term("title", "brown")]
    );
    assert_eq!(q.minimum_should_match, 0);
    let q = built(r#"{"query":{"match":{"title":{"query":"quick brown","operator":"and"}}}}"#)
        .expect("builds");
    assert_eq!(q.must, vec![term("title", "quick"), term("title", "brown")]);
    let q = built(r#"{"query":{"match":{"title":{"query":"quick brown fox snow","minimum_should_match":"75%"}}}}"#)
        .expect("builds");
    assert_eq!(q.minimum_should_match, 3);
    assert_eq!(
        one(r#"{"query":{"match":{"title":{"query":"quick","minimum_should_match":"2"}}}}"#),
        term("title", "quick"),
        "one term is not a boolean query, so no minimum applies"
    );
    assert_eq!(
        one(r#"{"query":{"match":{"tag":"food"}}}"#),
        keyword("tag", "food")
    );
    assert_eq!(
        one(r#"{"query":{"match":{"year":1999}}}"#),
        ints("year", 1999, 1999)
    );
    assert_eq!(one(r#"{"query":{"match":{"title":"!!"}}}"#), none());
    assert_eq!(
        one(r#"{"query":{"match":{"title":{"query":"!!","zero_terms_query":"all"}}}}"#),
        all()
    );
    assert_eq!(one(r#"{"query":{"match":{"nope":"x"}}}"#), none());
}

#[test]
fn ranges_round_their_bounds() {
    assert_eq!(
        one(r#"{"query":{"range":{"year":{"gt":1999,"lt":2010}}}}"#),
        ints("year", 2000, 2009)
    );
    assert_eq!(
        one(r#"{"query":{"range":{"year":{"gt":2010,"lt":2011}}}}"#),
        none()
    );
    assert_eq!(
        one(r#"{"query":{"range":{"n":{"gte":5}}}}"#),
        longs("n", 5, i64::MAX)
    );
    assert_eq!(one(r#"{"query":{"range":{"n":{"gt":5,"lt":6}}}}"#), none());
    let start = date::parse_to_millis("2002-07-01", false).expect("date");
    assert_eq!(
        one(r#"{"query":{"range":{"published":{"gt":"2002-07-01"}}}}"#),
        longs("published", start + 86_400_000, i64::MAX)
    );
    assert_eq!(
        one(r#"{"query":{"range":{"published":{"lte":946598400000}}}}"#),
        longs("published", i64::MIN, 946_598_400_000)
    );
    assert_eq!(
        one(r#"{"query":{"range":{"published":{"gt":"2002-07-02","lt":"2002-07-01"}}}}"#),
        none()
    );
    assert_eq!(one(r#"{"query":{"range":{"nope":{"gte":1}}}}"#), none());
}

#[test]
fn bool_queries_keep_opensearch_s_rules() {
    assert_eq!(one(r#"{"query":{"bool":{}}}"#), all());
    let q = built(r#"{"query":{"bool":{"must_not":{"term":{"tag":"animal"}}}}}"#).expect("builds");
    assert_eq!(q.must_not, vec![keyword("tag", "animal")]);
    assert_eq!(
        q.filter,
        vec![all()],
        "a purely negative query matches the rest"
    );
    let q = built(
        r#"{"query":{"bool":{"must_not":{"term":{"tag":"a"}},"adjust_pure_negative":false}}}"#,
    )
    .expect("builds");
    assert!(q.filter.is_empty());
    let q = built(
        r#"{"query":{"bool":{"should":[{"term":{"tag":"a"}},{"term":{"tag":"b"}},{"term":{"tag":"c"}}],"minimum_should_match":"-1","filter":{"term":{"year":1}}}}}"#,
    )
    .expect("builds");
    assert_eq!(q.minimum_should_match, 2);
    assert_eq!(q.filter, vec![ints("year", 1, 1)]);
    let q = built(r#"{"query":{"bool":{"must":{"match":{"title":"quick"}},"boost":2}}}"#)
        .expect("builds");
    assert!(
        matches!(&q.must[..], [Clause::Boost(b)] if b.boost == 2.0),
        "{q:?}"
    );
}

#[test]
fn failures_are_the_exceptions_java_throws() {
    assert_eq!(
        reason(r#"{"query":{"term":{"year":"abc"}}}"#),
        "For input string: \"abc\""
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"year":3000000000}}}"#),
        "Value [3000000000] is out of range for an integer"
    );
    assert!(
        reason(r#"{"query":{"range":{"published":{"gte":"abc"}}}}"#)
            .starts_with("failed to parse date field [abc]")
    );
    assert!(
        reason(r#"{"query":{"range":{"published":{"lt":"abc"}}}}"#)
            .starts_with("failed to parse date field [abc]")
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"geo":"x"}}}"#),
        "searching a [geo_point] field [geo] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"hidden":"x"}}}"#),
        "searching the field [hidden], which is not indexed, is not supported by this node yet"
    );
    assert_eq!(
        reason(r#"{"query":{"range":{"tag":{"gte":"a"}}}}"#),
        "a range query on the string field [tag] is not supported by this node yet"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"title":{"query":"a b","minimum_should_match":"x"}}}}"#),
        "For input string: \"x\""
    );
    assert_eq!(
        reason(r#"{"query":{"bool":{"should":{"term":{"year":"x"}}}}}"#),
        "For input string: \"x\""
    );
}

#[test]
fn a_plain_failure_is_wrapped_as_a_query_shard_exception() {
    let f = shard_failure(Failure::plain("number_format_exception", "x"), "i", "u");
    assert_eq!(f.kind, "query_shard_exception");
    assert_eq!(f.reason, "failed to create query: x");
    assert_eq!(
        f.meta,
        vec![
            ("index", Meta::Str("i".into())),
            ("index_uuid", Meta::Str("u".into()))
        ]
    );
    assert_eq!(
        f.cause.as_deref().map(|c| c.kind),
        Some("number_format_exception")
    );
    let parse = Failure::opensearch("parse_exception", "y");
    assert_eq!(shard_failure(parse.clone(), "i", "u"), parse);
}

#[test]
fn minimum_should_match_specs() {
    let m = |n, s| calculate_min_should_match(n, s).ok();
    assert_eq!(m(3, "2"), Some(2));
    assert_eq!(m(3, "-1"), Some(2));
    assert_eq!(m(2, "5"), Some(5), "not capped at the clause count");
    assert_eq!(m(3, "-5"), Some(0));
    assert_eq!(m(4, "75%"), Some(3));
    assert_eq!(m(3, "75%"), Some(2));
    assert_eq!(m(3, "-25%"), Some(3));
    assert_eq!(m(4, "-25%"), Some(3));
    assert_eq!(m(3, " 2 "), Some(2));
    assert_eq!(
        m(2, "3<90%"),
        Some(2),
        "at or under the bound, every clause"
    );
    assert_eq!(m(5, "3<90%"), Some(4));
    assert_eq!(m(5, "2<-25% 9<-3"), Some(4));
    assert_eq!(m(10, "2<-25% 9<-3"), Some(7));
    assert_eq!(
        m(10, "2 < -25%  9 <-3"),
        None,
        "two spaces leave an empty part"
    );
    assert_eq!(m(10, "2 < -25% 9 <-3"), Some(7));
    for bad in ["x", "x%", "%", "x<1", "3<x", "5%x"] {
        assert!(m(10, bad).is_none(), "{bad}");
    }
    let f = calculate_min_should_match(1, "5%x").expect_err("fails");
    assert_eq!(f.reason, "For input string: \"5%\"");
}

#[test]
fn now_in_a_date_range_is_the_instant_given_and_refused_without_one() {
    let now = date::parse_to_millis("2002-07-01T10:15:30Z", false).expect("date");
    let body = r#"{"query":{"range":{"published":{"gte":"now-1h","lt":"now/d+1d"}}}}"#;
    let q = built_at(body, Some(now)).expect("builds");
    let from = now - 3_600_000;
    let to = date::parse_to_millis("2002-07-02", false).expect("date") - 1;
    assert_eq!(q.must, vec![longs("published", from, to)]);
    let refused = built(body).expect_err("no instant");
    assert!(
        format!("{refused:?}").contains("not supported"),
        "{refused:?}"
    );
}
