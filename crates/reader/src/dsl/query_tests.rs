//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;
use crate::dsl::bool_query::BoolQuery;
use crate::dsl::parse_chunks;

fn query(body: &str) -> Result<Query, DslError> {
    parse_chunks([body.as_bytes()]).map(|s| s.query.expect("a query"))
}

fn failure(body: &str) -> crate::failure::Failure {
    match parse_chunks([body.as_bytes()]) {
        Err(e) => e.into_failure(),
        Ok(s) => panic!("parsed {s:?}"),
    }
}

fn reason(body: &str) -> String {
    failure(body).reason
}

fn term(field: &str, value: Value, boost: f32) -> Query {
    Query::Term {
        field: field.into(),
        value,
        boost,
    }
}

#[test]
fn match_all_and_its_boost() {
    assert_eq!(
        query(r#"{"query":{"match_all":{}}}"#).ok(),
        Some(Query::MatchAll { boost: 1.0 })
    );
    assert_eq!(
        query(r#"{"query":{"match_all":{"boost":2}}}"#).ok(),
        Some(Query::MatchAll { boost: 2.0 })
    );
    assert_eq!(
        reason(r#"{"query":{"match_all":{"bost":2}}}"#),
        "[1:31] [match_all] unknown field [bost] did you mean [boost]?"
    );
    assert!(reason(r#"{"query":{"match_all":{"_name":"x"}}}"#).contains("not supported"));
}

#[test]
fn term_short_and_long_forms() {
    assert_eq!(
        query(r#"{"query":{"term":{"tag":"animal"}}}"#).ok(),
        Some(term("tag", Value::Str("animal".into()), 1.0))
    );
    assert_eq!(
        query(r#"{"query":{"term":{"year":2002}}}"#).ok(),
        Some(term("year", Value::Number("2002".into()), 1.0))
    );
    assert_eq!(
        query(r#"{"query":{"term":{"tag":{"value":"manual","boost":2.0}}}}"#).ok(),
        Some(term("tag", Value::Str("manual".into()), 2.0))
    );
    assert_eq!(
        query(r#"{"query":{"term":{"tag":{"term":true,"case_insensitive":false}}}}"#).ok(),
        Some(term("tag", Value::Bool(true), 1.0))
    );
}

#[test]
fn term_errors_are_opensearchs() {
    let f = failure(r#"{"query":{"term":{"tag":{"value":"x","nope":1}}}}"#);
    assert_eq!(
        (f.kind, f.reason.as_str()),
        ("parsing_exception", "[term] query does not support [nope]")
    );
    assert_eq!(
        f.meta,
        [
            ("line", crate::failure::Meta::Int(1)),
            ("col", crate::failure::Meta::Int(45))
        ]
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"a":"x","b":"y"}}}"#),
        "[term] query doesn't support multiple fields, found [a] and [b]"
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"a":"x","b":{"value":"y"}}}}"#),
        "[term] query doesn't support multiple fields, found [a] and [b]"
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"a":["x"]}}}"#),
        "[term] query does not support array of values"
    );
    assert_eq!(
        reason(r#"{"query":{"term":{"a":null}}}"#),
        "value cannot be null"
    );
    assert_eq!(
        reason(r#"{"query":{"term":{}}}"#),
        "field name is null or empty"
    );
    assert!(
        reason(r#"{"query":{"term":{"a":{"value":"x","case_insensitive":true}}}}"#)
            .contains("not supported")
    );
    assert!(
        reason(r#"{"query":{"term":{"a":{"value":"x","_name":"n"}}}}"#).contains("not supported")
    );
}

#[test]
fn match_parses_every_served_parameter() {
    assert_eq!(
        query(r#"{"query":{"match":{"title":"quick brown"}}}"#).ok(),
        Some(Query::Match {
            field: "title".into(),
            query: Value::Str("quick brown".into()),
            operator: Operator::Or,
            minimum_should_match: None,
            zero_terms_all: false,
            boost: 1.0,
        })
    );
    assert_eq!(
        query(
            r#"{"query":{"match":{"title":{"query":"q","operator":"AND","minimum_should_match":"75%","zero_terms_query":"ALL","boost":3,"analyzer":"standard"}}}}"#
        )
        .ok(),
        Some(Query::Match {
            field: "title".into(),
            query: Value::Str("q".into()),
            operator: Operator::And,
            minimum_should_match: Some("75%".into()),
            zero_terms_all: true,
            boost: 3.0,
        })
    );
    let none =
        query(r#"{"query":{"match":{"t":{"query":1,"zero_terms_query":"none","operator":"or"}}}}"#);
    assert!(matches!(
        none,
        Ok(Query::Match {
            zero_terms_all: false,
            operator: Operator::Or,
            ..
        })
    ));
}

#[test]
fn match_errors_are_opensearchs() {
    assert_eq!(
        reason(r#"{"query":{"match":{"t":{"query":"x","nope":1}}}}"#),
        "[match] query does not support [nope]"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"t":{"query":{}}}}}"#),
        "[match] unknown token [START_OBJECT] after [query]"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"t":{"boost":1}}}}"#),
        "No text specified for text query"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"a":"x","b":"y"}}}"#),
        "[match] query doesn't support multiple fields, found [a] and [b]"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"a":"x","b":{"query":"y"}}}}"#),
        "[match] query doesn't support multiple fields, found [a] and [b]"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"t":{"query":"x","zero_terms_query":"some"}}}}"#),
        "Unsupported zero_terms_query value [some]"
    );
    assert_eq!(
        reason(r#"{"query":{"match":{"t":{"query":"x","operator":"xor"}}}}"#),
        "No enum constant org.opensearch.index.query.Operator.XOR"
    );
    for p in ["fuzziness", "lenient", "_name"] {
        let body = format!(r#"{{"query":{{"match":{{"t":{{"query":"x","{p}":"1"}}}}}}}}"#);
        assert!(reason(&body).contains("not supported"), "{p}");
    }
    assert!(
        reason(r#"{"query":{"match":{"t":{"query":"x","analyzer":"english"}}}}"#)
            .contains("not supported")
    );
}

#[test]
fn range_bounds_and_their_errors() {
    let r = |from: Option<&str>, to: Option<&str>, il, iu, boost| Query::Range {
        field: "year".into(),
        from: from.map(|s| Value::Number(s.into())),
        to: to.map(|s| Value::Number(s.into())),
        include_lower: il,
        include_upper: iu,
        boost,
        date: DateSpec::default(),
    };
    assert_eq!(
        query(r#"{"query":{"range":{"year":{"gte":1,"lte":2}}}}"#).ok(),
        Some(r(Some("1"), Some("2"), true, true, 1.0))
    );
    assert_eq!(
        query(r#"{"query":{"range":{"year":{"gt":1,"lt":2,"boost":2}}}}"#).ok(),
        Some(r(Some("1"), Some("2"), false, false, 2.0))
    );
    assert_eq!(
        query(r#"{"query":{"range":{"year":{"from":1,"to":2,"include_lower":false,"include_upper":false}}}}"#).ok(),
        Some(r(Some("1"), Some("2"), false, false, 1.0))
    );
    assert_eq!(
        query(r#"{"query":{"range":{"year":{}}}}"#).ok(),
        Some(r(None, None, true, true, 1.0))
    );
    for body in [
        r#"{"query":{"range":{"y":{"gt":1,"gte":2}}}}"#,
        r#"{"query":{"range":{"y":{"from":1,"from":2}}}}"#,
        r#"{"query":{"range":{"y":{"include_lower":true,"include_lower":true}}}}"#,
        r#"{"query":{"range":{"y":{"include_lower":true,"gt":1}}}}"#,
    ] {
        assert_eq!(
            reason(body),
            "invalid lower bound for [range] query",
            "{body}"
        );
    }
    for body in [
        r#"{"query":{"range":{"y":{"lt":1,"lte":2}}}}"#,
        r#"{"query":{"range":{"y":{"to":1,"to":2}}}}"#,
        r#"{"query":{"range":{"y":{"include_upper":true,"include_upper":true}}}}"#,
    ] {
        assert_eq!(
            reason(body),
            "invalid upper bound for [range] query",
            "{body}"
        );
    }
    assert_eq!(
        reason(r#"{"query":{"range":{"y":{"nope":1}}}}"#),
        "[range] query does not support [nope]"
    );
    assert_eq!(
        reason(r#"{"query":{"range":{"y":1}}}"#),
        "[range] query does not support [y]"
    );
    assert_eq!(
        reason(r#"{"query":{"range":{"a":{},"b":{}}}}"#),
        "[range] query doesn't support multiple fields, found [a] and [b]"
    );
    assert!(reason(r#"{"query":{"range":{"y":{"format":"x"}}}}"#).contains("not supported"));
}

#[test]
fn bool_clauses_in_every_form() {
    let q = query(
        r#"{"query":{"bool":{"must":{"match_all":{}},"should":[{"term":{"a":"x"}},{"term":{"b":"y"}}],"filter":null,"must_not":[],"minimum_should_match":1,"boost":2,"adjust_pure_negative":false}}}"#,
    );
    let Ok(Query::Bool(b)) = q else {
        panic!("{q:?}")
    };
    assert_eq!(
        *b,
        BoolQuery {
            must: vec![Query::MatchAll { boost: 1.0 }],
            should: vec![
                term("a", Value::Str("x".into()), 1.0),
                term("b", Value::Str("y".into()), 1.0)
            ],
            must_not: vec![],
            filter: vec![],
            minimum_should_match: Some("1".into()),
            adjust_pure_negative: false,
            boost: 2.0,
        }
    );
    let Ok(Query::Bool(b)) = query(r#"{"query":{"bool":{"minimum_should_match":"50%"}}}"#) else {
        panic!("bool");
    };
    assert_eq!(b.minimum_should_match.as_deref(), Some("50%"));
    assert!(b.adjust_pure_negative);
}

#[test]
fn bool_errors_are_object_parsers() {
    let f = failure(r#"{"query":{"bool":{"nope":[]}}}"#);
    assert_eq!(
        (f.kind, f.reason.as_str()),
        (
            "x_content_parse_exception",
            "[1:19] [bool] unknown field [nope]"
        )
    );
    assert_eq!(
        reason(r#"{"query":{"bool":{"mst":[]}}}"#),
        "[1:19] [bool] unknown field [mst] did you mean [must]?"
    );
    assert_eq!(
        reason(r#"{"query":{"bool":{"must":"x"}}}"#),
        "[1:26] [bool] must doesn't support values of type: VALUE_STRING"
    );
    assert_eq!(
        reason(r#"{"query":{"bool":{"boost":{}}}}"#),
        "[1:27] [bool] boost doesn't support values of type: START_OBJECT"
    );
    let wrapped = failure(r#"{"query":{"bool":{"must":[{"nope":{}}]}}}"#);
    assert_eq!(wrapped.reason, "[1:35] [bool] failed to parse field [must]");
    let cause = wrapped.cause.expect("the inner failure");
    assert_eq!(cause.reason, "unknown query [nope]");
    assert!(
        reason(r#"{"query":{"bool":{"_name":"n"}}}"#)
            .contains("[bool] failed to parse field [_name]")
    );
    let many = format!(
        r#"{{"query":{{"bool":{{"should":[{}]}}}}}}"#,
        vec![r#"{"match_all":{}}"#; crate::dsl::bool_query::MAX_CLAUSES + 1].join(",")
    );
    let f = failure(&many);
    assert!(
        f.cause
            .expect("cause")
            .reason
            .starts_with("too many clauses")
    );
}

#[test]
fn malformed_query_objects() {
    assert_eq!(
        failure(r#"{"query":{}}"#).reason,
        "query malformed, empty clause found at [1:11]"
    );
    let unknown = failure(r#"{"query":{"no_such_query":{}}}"#);
    assert_eq!(unknown.reason, "unknown query [no_such_query]");
    assert_eq!(
        unknown.cause.map(|c| c.reason),
        Some("[1:27] unknown field [no_such_query]".to_owned())
    );
    assert_eq!(
        reason(r#"{"query":{"matc":{}}}"#),
        "unknown query [matc] did you mean [match]?"
    );
    assert!(reason(r#"{"query":{"span_term":{}}}"#).contains("[span_term] query is not supported"));
    assert_eq!(
        reason(r#"{"query":{"match_all":1}}"#),
        "[match_all] query malformed, no start_object after query name"
    );
    assert_eq!(
        reason(r#"{"query":{"match_all":{},"term":{}}}"#),
        "[match_all] malformed query, expected [END_OBJECT] but found [FIELD_NAME]"
    );
    assert_eq!(
        reason(r#"{"query":{"bool":{"must":[1]}}}"#),
        "[1:28] [bool] failed to parse field [must]"
    );
    let inner = failure(r#"{"query":{"bool":{"must":[1]}}}"#)
        .cause
        .expect("cause");
    assert_eq!(
        inner.reason,
        "[_na] query malformed, must start with start_object"
    );
}

#[test]
fn a_range_reads_its_format_and_zone_and_refuses_what_it_cannot() {
    use crate::date::DateFormat;
    let date = |inner: &str| match query(&format!(
        r#"{{"query":{{"range":{{"d":{{"gte":"2001",{inner}}}}}}}}}"#
    ))
    .expect("parses")
    {
        Query::Range { date, .. } => date,
        other => panic!("{other:?}"),
    };
    assert_eq!(date(r#""time_zone":"+01:30""#).zone, 5_400);
    assert!(date(r#""time_zone":"UTC""#).named);
    assert_eq!(date(r#""format":"epoch_millis""#).format, DateFormat::Epoch);
    assert_eq!(
        date(r#""format":"strict_date_optional_time""#).format,
        DateFormat::Iso
    );
    assert_eq!(date(r#""boost":2"#), DateSpec::default());
    for bad in [
        r#""time_zone":"Asia/Kolkata""#,
        r#""time_zone":5"#,
        r#""format":"yyyy-MM-dd""#,
        r#""format":true"#,
    ] {
        let q = format!(r#"{{"query":{{"range":{{"d":{{"gte":"2001",{bad}}}}}}}}}"#);
        let reason = query(&q).expect_err("refused").into_failure().reason;
        assert!(
            reason.ends_with("is not supported by this node yet"),
            "{bad}: {reason}"
        );
    }
}
