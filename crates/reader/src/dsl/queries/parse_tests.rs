//! Tests of the parsers; compiled only under `#[cfg(test)]`.

use super::*;
use crate::dsl::parse_chunks;
use crate::dsl::queries::case_insensitive::Kind;
use crate::failure::Failure;

fn more(body: &str) -> More {
    match parse_chunks([body.as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => *m,
        other => panic!("parsed {other:?}"),
    }
}

fn failure(body: &str) -> Failure {
    match parse_chunks([body.as_bytes()]) {
        Err(e) => e.into_failure(),
        Ok(s) => panic!("parsed {s:?}"),
    }
}

fn reason(body: &str) -> String {
    failure(body).reason
}

fn q(inner: &str) -> String {
    format!(r#"{{"query":{inner}}}"#)
}

fn s(text: &str) -> Value {
    Value::Str(text.into())
}

#[test]
fn terms_reads_values_and_a_boost() {
    assert_eq!(
        more(&q(r#"{"terms":{"tag":["a",2,true],"boost":2}}"#)),
        More::Terms {
            field: "tag".into(),
            values: vec![s("a"), Value::Number("2".into()), Value::Bool(true)],
            boost: 2.0,
        }
    );
    assert_eq!(
        more(&q(r#"{"terms":{"tag":[],"value_type":"default"}}"#)),
        More::Terms {
            field: "tag".into(),
            values: vec![],
            boost: 1.0,
        }
    );
}

#[test]
fn terms_refuses_what_opensearch_refuses() {
    assert_eq!(
        reason(&q(r#"{"terms":{"tag":["a"],"title":["b"]}}"#)),
        "[terms] query does not support multiple fields"
    );
    assert_eq!(
        reason(&q(r#"{"terms":{"tag":["a"],"title":{"id":"1"}}}"#)),
        "[terms] query does not support more than one field. Already got: [tag] but also found [title]"
    );
    assert_eq!(
        reason(&q(r#"{"terms":{"tag":[null]}}"#)),
        "No value specified for terms query"
    );
    assert_eq!(
        reason(&q(r#"{"terms":{"boost":2}}"#)),
        "[terms] query requires a field name, followed by array of terms or a document lookup specification"
    );
    assert_eq!(
        reason(&q(r#"{"terms":{"tag":["a"],"x":1}}"#)),
        "[terms] query does not support [x]"
    );
    for unsupported in [
        r#"{"terms":{"tag":{"index":"i","id":"1","path":"p"}}}"#,
        r#"{"terms":{"tag":[["a"]]}}"#,
        r#"{"terms":{"tag":["a"],"value_type":"bitmap"}}"#,
    ] {
        assert!(
            reason(&q(unsupported)).contains("not supported"),
            "{unsupported}"
        );
    }
}

#[test]
fn prefix_wildcard_and_regexp_short_and_long_forms() {
    assert_eq!(
        more(&q(r#"{"prefix":{"tag":"an"}}"#)),
        More::Prefix {
            field: "tag".into(),
            value: "an".into(),
            boost: 1.0
        }
    );
    assert_eq!(
        more(&q(
            r#"{"prefix":{"tag":{"value":"an","boost":2,"case_insensitive":false,"rewrite":null}}}"#
        )),
        More::Prefix {
            field: "tag".into(),
            value: "an".into(),
            boost: 2.0
        }
    );
    assert_eq!(
        more(&q(r#"{"wildcard":{"tag":{"wildcard":"a*"}}}"#)),
        More::Wildcard {
            field: "tag".into(),
            value: "a*".into(),
            boost: 1.0
        }
    );
    assert_eq!(
        more(&q(r#"{"wildcard":{"tag":"a?"}}"#)),
        More::Wildcard {
            field: "tag".into(),
            value: "a?".into(),
            boost: 1.0
        }
    );
    assert_eq!(
        more(&q(
            r#"{"regexp":{"tag":{"value":"a.*","flags":"ALL","flags_value":65535,"max_determinized_states":10000,"boost":3}}}"#
        )),
        More::Regexp {
            field: "tag".into(),
            value: "a.*".into(),
            boost: 3.0
        }
    );
    assert_eq!(
        more(&q(r#"{"regexp":{"tag":"a.*"}}"#)),
        More::Regexp {
            field: "tag".into(),
            value: "a.*".into(),
            boost: 1.0
        }
    );
}

#[test]
fn string_queries_refuse_bad_shapes_and_unserved_options() {
    assert_eq!(
        reason(&q(r#"{"prefix":{"tag":"a","title":"b"}}"#)),
        "[prefix] query doesn't support multiple fields, found [tag] and [title]"
    );
    assert_eq!(
        reason(&q(
            r#"{"wildcard":{"tag":{"value":"a"},"title":{"value":"b"}}}"#
        )),
        "[wildcard] query doesn't support multiple fields, found [tag] and [title]"
    );
    assert_eq!(
        reason(&q(r#"{"prefix":{"tag":{"vale":"a"}}}"#)),
        "[prefix] query does not support [vale]"
    );
    assert_eq!(
        reason(&q(r#"{"regexp":{"tag":{"value":"a","x":1}}}"#)),
        "[regexp] query does not support [x]"
    );
    assert_eq!(
        reason(&q(r#"{"wildcard":{"tag":{"x":1}}}"#)),
        "[wildcard] query does not support [x]"
    );
    assert_eq!(
        reason(&q(r#"{"prefix":{"tag":{"value":{"a":1}}}}"#)),
        "[prefix] unexpected token [START_OBJECT] after [value]"
    );
    assert_eq!(
        reason(&q(r#"{"prefix":{"tag":["a"]}}"#)),
        "[prefix] unexpected token [START_ARRAY] after [tag]"
    );
    assert_eq!(
        reason(&q(r#"{"prefix":{}}"#)),
        "field name is null or empty"
    );
    assert_eq!(
        reason(&q(r#"{"prefix":{"tag":null}}"#)),
        "value cannot be null"
    );
    assert_eq!(
        reason(&q(r#"{"regexp":{"tag":{"boost":2}}}"#)),
        "value cannot be null"
    );
    assert_eq!(
        reason(&q(
            r#"{"regexp":{"tag":{"value":"a","max_determinized_states":-1}}}"#
        )),
        "[regexp] max_determinized_states cannot be negative but was [-1]"
    );
    for unsupported in [
        r#"{"regexp":{"tag":{"value":"a","case_insensitive":true}}}"#,
        r#"{"prefix":{"tag":{"value":"a","rewrite":"scoring_boolean"}}}"#,
        r#"{"regexp":{"tag":{"value":"a","flags":"INTERSECTION"}}}"#,
        r#"{"regexp":{"tag":{"value":"a","flags_value":1}}}"#,
        r#"{"regexp":{"tag":{"value":"a","max_determinized_states":100}}}"#,
        r#"{"regexp":{"tag":{"value":"a","max_determinized_states":0}}}"#,
    ] {
        assert!(
            reason(&q(unsupported)).contains("not supported"),
            "{unsupported}"
        );
    }
}

#[test]
fn fuzzy_reads_every_parameter() {
    assert_eq!(
        more(&q(
            r#"{"fuzzy":{"title":{"value":"fox","fuzziness":"AUTO:2,4","prefix_length":1,"max_expansions":5,"transpositions":false,"boost":2}}}"#
        )),
        More::Fuzzy {
            field: "title".into(),
            value: s("fox"),
            fuzziness: Fuzziness::Auto { low: 2, high: 4 },
            prefix_length: 1,
            max_expansions: 5,
            transpositions: false,
            boost: 2.0,
        }
    );
    assert_eq!(
        more(&q(r#"{"fuzzy":{"title":"fox"}}"#)),
        More::Fuzzy {
            field: "title".into(),
            value: s("fox"),
            fuzziness: Fuzziness::AUTO,
            prefix_length: 0,
            max_expansions: 50,
            transpositions: true,
            boost: 1.0,
        }
    );
    let fuzziness = |f: &str| match more(&q(&format!(
        r#"{{"fuzzy":{{"title":{{"term":"fox","fuzziness":{f}}}}}}}"#
    ))) {
        More::Fuzzy { fuzziness, .. } => fuzziness,
        other => panic!("{other:?}"),
    };
    assert_eq!(fuzziness(r#""auto""#), Fuzziness::AUTO);
    assert_eq!(fuzziness("0"), Fuzziness::Edits(0));
    assert_eq!(fuzziness(r#""1""#), Fuzziness::Edits(1));
    assert_eq!(fuzziness("7"), Fuzziness::Edits(2));
    assert_eq!(fuzziness("1.5"), Fuzziness::Edits(1));
    assert_eq!(fuzziness("2.5"), Fuzziness::Edits(2));
}

#[test]
fn fuzzy_refuses_what_opensearch_refuses() {
    let bad = |f: &str| {
        reason(&q(&format!(
            r#"{{"fuzzy":{{"title":{{"value":"fox","fuzziness":{f}}}}}}}"#
        )))
    };
    assert_eq!(bad(r#""lots""#), "Invalid fuzziness value: lots");
    assert_eq!(bad("-1"), "Invalid fuzziness value: -1");
    assert_eq!(bad("-0.5"), "Invalid fuzziness value: -0.5");
    assert_eq!(
        bad("true"),
        "Can't parse fuzziness on token: [VALUE_BOOLEAN]"
    );
    assert_eq!(
        bad(r#""AUTO:1""#),
        "failed to find low and high distance values"
    );
    assert_eq!(
        bad(r#""AUTO:a,b""#),
        "failed to parse [AUTO:a,b] as a \"auto:int,int\""
    );
    assert_eq!(
        bad(r#""AUTO:""#),
        "failed to find low and high distance values"
    );
    assert_eq!(
        bad(r#""AUTO:1,2,3""#),
        "failed to find low and high distance values"
    );
    for fine in [
        r#""AUTO:0,4""#,
        r#""AUTO:0,0""#,
        r#""AUTO:3,3""#,
        r#""auto:1,2,,""#,
    ] {
        assert!(
            matches!(
                more(&q(&format!(
                    r#"{{"fuzzy":{{"title":{{"value":"a","fuzziness":{fine}}}}}}}"#
                ))),
                More::Fuzzy { .. }
            ),
            "{fine}"
        );
    }
    for wrong in [r#""AUTO:-1,4""#, r#""AUTO:5,4""#] {
        assert_eq!(
            bad(wrong),
            "fuzziness wrongly configured, must be: lowDistance > 0, highDistance > 0 and \
             lowDistance <= highDistance ",
            "{wrong}"
        );
    }
    assert_eq!(
        reason(&q(r#"{"fuzzy":{"title":{"value":"fox","x":1}}}"#)),
        "[fuzzy] query does not support [x]"
    );
    assert_eq!(
        reason(&q(r#"{"fuzzy":{}}"#)),
        "field name cannot be null or empty"
    );
    assert_eq!(
        reason(&q(r#"{"fuzzy":{"title":{"boost":1}}}"#)),
        "query value cannot be null"
    );
    assert!(
        reason(&q(r#"{"fuzzy":{"title":{"value":"a","rewrite":"x"}}}"#)).contains("not supported")
    );
}

#[test]
fn exists_reads_its_field_and_refuses_the_rest() {
    assert_eq!(
        more(&q(r#"{"exists":{"field":"tag","boost":2}}"#)),
        More::Exists {
            field: "tag".into(),
            boost: 2.0
        }
    );
    assert_eq!(
        reason(&q(r#"{"exists":{}}"#)),
        "[exists] must be provided with a [field]"
    );
    assert_eq!(
        reason(&q(r#"{"exists":{"field":""}}"#)),
        "field name is null or empty"
    );
    assert_eq!(
        reason(&q(r#"{"exists":{"fields":"tag"}}"#)),
        "[exists] query does not support [fields]"
    );
    assert_eq!(
        reason(&q(r#"{"exists":{"field":["tag"]}}"#)),
        "[exists] unknown token [START_ARRAY] after [field]"
    );
    for named in [
        r#"{"exists":{"field":"tag","_name":"n"}}"#,
        r#"{"terms":{"tag":["a"],"_name":"n"}}"#,
        r#"{"wildcard":{"tag":{"value":"a","_name":"n"}}}"#,
    ] {
        assert!(
            matches!(
                parse_chunks([q(named).as_bytes()]).map(|s| s.query),
                Ok(Some(Query::Named(_)))
            ),
            "{named}"
        );
    }
}

#[test]
fn ids_reads_values_and_wraps_its_failures_as_parsing() {
    assert_eq!(
        more(&q(r#"{"ids":{"values":["1",2],"boost":3}}"#)),
        More::Ids {
            values: vec!["1".into(), "2".into()],
            boost: 3.0
        }
    );
    let f = failure(&q(r#"{"ids":{"value":["1"]}}"#));
    assert_eq!(f.kind, "parsing_exception");
    assert_eq!(
        f.reason,
        "[1:18] [ids] unknown field [value] did you mean [values]?"
    );
    assert_eq!(f.cause.map(|c| c.kind), Some("x_content_parse_exception"));
    assert_eq!(
        reason(&q(r#"{"ids":{"values":[{"a":1}]}}"#)),
        "[1:28] [ids] values doesn't support values of type: START_OBJECT"
    );
    assert_eq!(
        reason(&q(r#"{"ids":{"values":"1"}}"#)),
        "[1:27] [ids] values doesn't support values of type: VALUE_STRING"
    );
    assert_eq!(
        reason(&q(r#"{"ids":{"boost":"x"}}"#)),
        "For input string: \"x\""
    );
    assert!(matches!(
        parse_chunks([q(r#"{"ids":{"_name":"n"}}"#).as_bytes()]).map(|s| s.query),
        Ok(Some(Query::Named(_)))
    ));
}

#[test]
fn auto_fuzziness_follows_the_term_length_in_code_points() {
    assert_eq!(Fuzziness::AUTO.as_distance("ab"), 0);
    assert_eq!(Fuzziness::AUTO.as_distance("abc"), 1);
    assert_eq!(Fuzziness::AUTO.as_distance("abcde"), 1);
    assert_eq!(Fuzziness::AUTO.as_distance("abcdef"), 2);
    assert_eq!(Fuzziness::AUTO.as_distance("ééé"), 1);
    assert_eq!(Fuzziness::Edits(1).as_distance("abcdefgh"), 1);
}

#[test]
fn every_query_reports_its_boost() {
    for (body, boost) in [
        (r#"{"terms":{"t":["a"],"boost":2}}"#, 2.0),
        (r#"{"prefix":{"t":{"value":"a","boost":3}}}"#, 3.0),
        (r#"{"wildcard":{"t":{"value":"a","boost":4}}}"#, 4.0),
        (r#"{"regexp":{"t":{"value":"a","boost":5}}}"#, 5.0),
        (r#"{"fuzzy":{"t":{"value":"a","boost":6}}}"#, 6.0),
        (r#"{"exists":{"field":"t","boost":7}}"#, 7.0),
        (r#"{"ids":{"values":["1"],"boost":8}}"#, 8.0),
        (
            r#"{"constant_score":{"filter":{"match_all":{}},"boost":9}}"#,
            9.0,
        ),
        (
            r#"{"dis_max":{"queries":[{"match_all":{}}],"boost":10}}"#,
            10.0,
        ),
        (r#"{"match_none":{"boost":11}}"#, 11.0),
    ] {
        assert_eq!(more(&q(body)).boost(), boost, "{body}");
    }
}

#[test]
fn case_insensitive_term_level_queries_are_read() {
    for (body, kind) in [
        (
            r#"{"prefix":{"tag":{"value":"a","boost":2,"case_insensitive":true}}}"#,
            Kind::Prefix,
        ),
        (
            r#"{"wildcard":{"tag":{"value":"a*","case_insensitive":true}}}"#,
            Kind::Wildcard,
        ),
    ] {
        let m = more(&q(body));
        assert!(
            matches!(m, More::CaseInsensitive { kind: k, .. } if k == kind),
            "{body}: {m:?}"
        );
    }
}
