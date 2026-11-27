//! Tests of `function_score`'s parser; compiled only under `#[cfg(test)]`.
//! The messages were recorded from OpenSearch 3.10.0; the ones that depend
//! on the order of the body's keys are checked here on the bytes sent.

use lucene_engine::function_score::{Combine, Decay, Modifier, MultiValue, ScoreMode};

use super::function_score::{Entry, Function, FunctionScore};
use super::*;
use crate::dsl::parse_chunks;

fn parse(body: &str) -> Result<(FunctionScore, f32), crate::failure::Failure> {
    let text = format!(r#"{{"query":{{"function_score":{body}}}}}"#);
    match parse_chunks([text.as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => match *m {
            More::FunctionScore(q, boost) => Ok((*q, boost)),
            other => panic!("parsed {other:?}"),
        },
        Ok(other) => panic!("parsed {other:?}"),
        Err(e) => Err(e.into_failure()),
    }
}

fn parsed(body: &str) -> (FunctionScore, f32) {
    parse(body).expect("parses")
}

fn reason(body: &str) -> String {
    parse(body).expect_err("fails").reason
}

#[test]
fn the_defaults_are_those_of_opensearch() {
    let (q, boost) = parsed(r#"{"functions":[{"weight":2}]}"#);
    assert_eq!(q.query, None);
    assert_eq!(q.score_mode, ScoreMode::Multiply);
    assert_eq!(q.boost_mode, Combine::Multiply);
    assert_eq!(q.max_boost, f32::MAX);
    assert_eq!(boost, 1.0);
    assert_eq!(
        q.functions,
        vec![Entry {
            filter: None,
            weight: Some(2.0),
            function: None
        }]
    );
    assert!(parsed("{}").0.functions.is_empty());
}

#[test]
fn a_single_function_is_one_entry() {
    let (q, _) = parsed(r#"{"weight":3}"#);
    assert_eq!(q.functions.len(), 1);
    assert_eq!(q.functions[0].weight, Some(3.0));
    let (q, _) = parsed(r#"{"field_value_factor":{"field":"year"}}"#);
    assert_eq!(
        q.functions[0].function,
        Some(Function::FieldValue {
            field: "year".into(),
            factor: 1.0,
            modifier: Modifier::None,
            missing: None
        })
    );
}

#[test]
fn every_key_is_read() {
    let (q, boost) = parsed(
        r#"{"query":{"match_all":{}},"boost":2,"score_mode":"SUM","boost_mode":"Replace",
            "max_boost":1.5,"functions":[{"filter":{"match_all":{}},"weight":2,
            "field_value_factor":{"field":"year","factor":0.5,"modifier":"LOG1P","missing":3}}]}"#,
    );
    assert!(q.query.is_some());
    assert_eq!(
        (q.score_mode, q.boost_mode),
        (ScoreMode::Sum, Combine::Replace)
    );
    assert_eq!((q.max_boost, boost), (1.5, 2.0));
    let e = &q.functions[0];
    assert!(e.filter.is_some());
    assert_eq!(e.weight, Some(2.0));
    assert_eq!(
        e.function,
        Some(Function::FieldValue {
            field: "year".into(),
            factor: 0.5,
            modifier: Modifier::Log1p,
            missing: Some(3.0)
        })
    );
}

#[test]
fn the_modes_and_modifiers_have_their_names() {
    for (name, mode) in [
        ("multiply", ScoreMode::Multiply),
        ("first", ScoreMode::First),
        ("avg", ScoreMode::Avg),
        ("max", ScoreMode::Max),
        ("sum", ScoreMode::Sum),
        ("min", ScoreMode::Min),
    ] {
        let (q, _) = parsed(&format!(r#"{{"weight":1,"score_mode":"{name}"}}"#));
        assert_eq!(q.score_mode, mode, "{name}");
    }
    for (name, mode) in [
        ("multiply", Combine::Multiply),
        ("replace", Combine::Replace),
        ("sum", Combine::Sum),
        ("avg", Combine::Avg),
        ("min", Combine::Min),
        ("max", Combine::Max),
    ] {
        let (q, _) = parsed(&format!(r#"{{"weight":1,"boost_mode":"{name}"}}"#));
        assert_eq!(q.boost_mode, mode, "{name}");
    }
    for (name, m) in [
        ("none", Modifier::None),
        ("log", Modifier::Log),
        ("log1p", Modifier::Log1p),
        ("log2p", Modifier::Log2p),
        ("ln", Modifier::Ln),
        ("ln1p", Modifier::Ln1p),
        ("ln2p", Modifier::Ln2p),
        ("square", Modifier::Square),
        ("sqrt", Modifier::Sqrt),
        ("reciprocal", Modifier::Reciprocal),
    ] {
        let (q, _) = parsed(&format!(
            r#"{{"field_value_factor":{{"field":"y","modifier":"{name}"}}}}"#
        ));
        let Some(Function::FieldValue { modifier, .. }) = &q.functions[0].function else {
            panic!("not a field value");
        };
        assert_eq!(*modifier, m, "{name}");
    }
}

#[test]
fn a_decay_keeps_its_parameters_for_the_build() {
    for (name, decay) in [
        ("gauss", Decay::Gauss),
        ("exp", Decay::Exp),
        ("linear", Decay::Linear),
    ] {
        let (q, _) = parsed(&format!(
            r#"{{"{name}":{{"year":{{"origin":2001,"scale":"5"}},"multi_value_mode":"max"}}}}"#
        ));
        let Some(Function::Decay {
            decay: d,
            field,
            params,
            mode,
        }) = &q.functions[0].function
        else {
            panic!("not a decay");
        };
        assert_eq!(
            (*d, field.as_str(), *mode),
            (decay, "year", MultiValue::Max)
        );
        assert_eq!(params.len(), 2);
    }
    for (name, mode) in [
        ("min", MultiValue::Min),
        ("avg", MultiValue::Avg),
        ("sum", MultiValue::Sum),
        ("median", MultiValue::Median),
    ] {
        let (q, _) = parsed(&format!(
            r#"{{"gauss":{{"year":{{"origin":1,"scale":1}},"multi_value_mode":"{name}"}}}}"#
        ));
        let Some(Function::Decay { mode: m, .. }) = &q.functions[0].function else {
            panic!("not a decay");
        };
        assert_eq!(*m, mode, "{name}");
    }
    let (q, _) = parsed(r#"{"gauss":{"year":{"origin":1,"scale":1}}}"#);
    let Some(Function::Decay { mode, .. }) = &q.functions[0].function else {
        panic!("not a decay");
    };
    assert_eq!(*mode, MultiValue::Min);
}

#[test]
fn a_function_error_is_opensearchs() {
    for (body, message) in [
        (
            r#"{"weight":1,"aaa":1}"#,
            "failed to parse [function_score] query. already found function [weight], now encountering [aaa]. use [functions] array if you want to define several functions.",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1}},"weight":3}"#,
            "failed to parse [function_score] query. already found function [gauss], now encountering [weight]. use [functions] array if you want to define several functions.",
        ),
        (
            r#"{"weight":1,"gauss":{"year":{"origin":1,"scale":1}}}"#,
            "failed to parse [function_score] query. already found function [weight], now encountering [gauss]. use [functions] array if you want to define several functions.",
        ),
        (
            r#"{"weight":1,"functions":[]}"#,
            "failed to parse [function_score] query. [you can either define [functions] array or a single function, not both. already found [weight], now encountering [functions].]",
        ),
        (
            r#"{"gauss":{"year":{"origin":1,"scale":1}},"functions":[]}"#,
            "failed to parse [function_score] query. [you can either define [functions] array or a single function, not both. already found [gauss], now encountering [functions].]",
        ),
        (
            r#"{"functions":[{"weight":1}],"filter":{"match_all":{}}}"#,
            "failed to parse [function_score] query. [you can either define [functions] array or a single function, not both. already found [functions] array, now encountering [filter].]",
        ),
        (
            r#"{"functions":[{"weight":1}],"weight":2}"#,
            "failed to parse [function_score] query. [you can either define [functions] array or a single function, not both. already found [functions] array, now encountering [weight].]",
        ),
        (
            r#"{"functions":[{"gauss":{"y":{"origin":1,"scale":1}},"exp":{"y":{"origin":1,"scale":1}}}]}"#,
            "failed to parse function_score functions. already found [gauss], now encountering [exp].",
        ),
        (
            r#"{"aaa":1}"#,
            "failed to parse [function_score] query. field [aaa] is not supported",
        ),
        (
            r#"{"aaa":[1]}"#,
            "failed to parse [function_score] query. array [aaa] is not supported",
        ),
        (
            r#"{"aaa":{"x":1}}"#,
            "unknown query [function_score] did you mean any of [random_score, script_score]?",
        ),
        (
            r#"{"functions":[{"weight":1,"aaa":1}]}"#,
            "failed to parse [function_score] query. field [aaa] is not supported",
        ),
        (
            r#"{"functions":[{"field_value_factor":5}]}"#,
            "failed to parse [function_score] query. field [field_value_factor] is not supported",
        ),
        (
            r#"{"functions":[{}]}"#,
            "failed to parse [function_score] query. an entry in functions list is missing a function.",
        ),
        (
            r#"{"functions":[{"filter":{"match_all":{}}}]}"#,
            "failed to parse [function_score] query. an entry in functions list is missing a function.",
        ),
        (
            r#"{"functions":[5]}"#,
            "failed to parse [START_OBJECT]. malformed query, expected a [VALUE_NUMBER] while parsing functions but got a [function_score] instead",
        ),
        (
            r#"{"functions":[{"weight":-1}]}"#,
            "[weight] cannot be negative for a filtering function",
        ),
        (
            r#"{"weight":-1}"#,
            "[weight] cannot be negative for a filtering function",
        ),
        (
            r#"{"weight":1,"score_mode":"bogus"}"#,
            "No enum constant org.opensearch.common.lucene.search.function.FunctionScoreQuery.ScoreMode.BOGUS",
        ),
        (
            r#"{"weight":1,"boost_mode":"bogus"}"#,
            "No enum constant org.opensearch.common.lucene.search.function.CombineFunction.BOGUS",
        ),
        (
            r#"{"field_value_factor":{"field":"y","modifier":"bogus"}}"#,
            "No enum constant org.opensearch.common.lucene.search.function.FieldValueFactorFunction.Modifier.BOGUS",
        ),
        (
            r#"{"gauss":{"y":{"origin":1,"scale":1},"multi_value_mode":"bogus"}}"#,
            "Illegal sort mode: bogus",
        ),
        (
            r#"{"field_value_factor":{"factor":2}}"#,
            "[field_value_factor] required field 'field' missing",
        ),
        (
            r#"{"field_value_factor":{"field":"y","aaa":1}}"#,
            "field_value_factor query does not support [aaa]",
        ),
        (
            r#"{"field_value_factor":{"field":"y","factor":[1]}}"#,
            "field_value_factor query does not support [factor]",
        ),
        (
            r#"{"field_value_factor":{"field":"y","aaa":{}}}"#,
            "field_value_factor query does not support [aaa]",
        ),
        (r#"{"weight":"x"}"#, "For input string: \"x\""),
        (r#"{"weight":1,"max_boost":"x"}"#, "For input string: \"x\""),
        (
            r#"{"field_value_factor":{"field":"y","missing":"x"}}"#,
            "For input string: \"x\"",
        ),
    ] {
        assert_eq!(reason(body), message, "{body}");
    }
}

#[test]
fn an_unknown_function_names_the_registrys_cause() {
    let f = parse(r#"{"gaus":{"year":{"origin":1,"scale":1}}}"#).expect_err("fails");
    assert_eq!(f.kind, "parsing_exception");
    let cause = f.cause.expect("cause");
    assert_eq!(cause.kind, "named_object_not_found_exception");
    assert!(
        cause.reason.ends_with("unknown field [gaus]"),
        "{}",
        cause.reason
    );
}

#[test]
fn what_is_not_served_is_refused() {
    for (body, what) in [
        (
            r#"{"random_score":{"_name":"r"}}"#,
            "[function_score] [_name]",
        ),
        (r#"{"weight":1,"_name":"q"}"#, "[function_score] [_name]"),
        (
            r#"{"weight":1,"boost":-1}"#,
            "[function_score] a negative [boost]",
        ),
        (r#"{"gauss":{}}"#, "[function_score] a decay of no field"),
        (
            r#"{"gauss":{"a":{"origin":1,"scale":1},"b":{"origin":1,"scale":1}}}"#,
            "[function_score] a decay's [b]",
        ),
        (
            r#"{"gauss":{"a":{"origin":null,"scale":1}}}"#,
            "[function_score] a null decay value",
        ),
        (
            r#"{"gauss":{"a":{"origin":{"lat":1},"scale":1}}}"#,
            "[function_score] the decay parameter [origin] as an object",
        ),
        (
            r#"{"gauss":{"a":{"origin":1,"scale":1},"aaa":1}}"#,
            "[function_score] a decay's [aaa]",
        ),
    ] {
        assert_eq!(
            reason(body),
            format!("{what} is not supported by this node yet"),
            "{body}"
        );
    }
}

#[test]
fn a_nested_query_is_parsed_with_its_errors() {
    assert!(reason(r#"{"query":{"nope":{}},"weight":1}"#).contains("unknown query [nope]"));
    assert!(
        reason(r#"{"functions":[{"filter":{"nope":{}},"weight":1}]}"#)
            .contains("unknown query [nope]")
    );
}

#[test]
fn the_boost_is_the_querys() {
    let m = More::FunctionScore(Box::new(parsed(r#"{"weight":1}"#).0), 4.0);
    assert_eq!(m.boost(), 4.0);
}

#[test]
fn a_zero_weight_and_a_zero_boost_are_served() {
    assert_eq!(parsed(r#"{"weight":0}"#).0.functions[0].weight, Some(0.0));
    assert_eq!(parsed(r#"{"weight":1,"boost":0}"#).1, 0.0);
}

#[test]
fn a_random_score_reads_its_seed_as_stock_does() {
    let seed = |body: &str| match parsed(body).0.functions.remove(0).function {
        Some(Function::Random { seed, field }) => (seed, field),
        other => panic!("{other:?}"),
    };
    assert_eq!(seed(r#"{"random_score":{}}"#), (None, None));
    assert_eq!(
        seed(r#"{"random_score":{"seed":-7,"field":"year"}}"#),
        (Some(-7), Some("year".into()))
    );
    // `Long.hashCode(3000000000)` and `"abc".hashCode()`.
    assert_eq!(
        seed(r#"{"random_score":{"seed":3000000000}}"#),
        (Some(-1_294_967_296), None)
    );
    assert_eq!(
        seed(r#"{"random_score":{"seed":"abc"}}"#),
        (Some(96_354), None)
    );
}

#[test]
fn a_random_score_refuses_what_stock_refuses() {
    for (body, message) in [
        (
            r#"{"random_score":{"seed":1.5}}"#,
            "random_score seed must be an int, long or string, not 'VALUE_NUMBER'",
        ),
        (
            r#"{"random_score":{"seed":99999999999999999999}}"#,
            "random_score seed must be an int, long or string, not 'VALUE_NUMBER'",
        ),
        (
            r#"{"random_score":{"seed":null}}"#,
            "random_score seed must be an int/long or string, not 'VALUE_NULL'",
        ),
        (
            r#"{"random_score":{"x":1}}"#,
            "random_score query does not support [x]",
        ),
    ] {
        assert_eq!(reason(body), message, "{body}");
    }
}

#[test]
fn a_min_score_is_read_and_defaults_to_none() {
    assert_eq!(parsed(r#"{"weight":1}"#).0.min_score, None);
    assert_eq!(
        parsed(r#"{"weight":1,"min_score":0.5}"#).0.min_score,
        Some(0.5)
    );
}

fn script_of(body: &str) -> super::function_score_script::ScriptScore {
    match parsed(body)
        .0
        .functions
        .into_iter()
        .next()
        .and_then(|e| e.function)
    {
        Some(Function::Script(s)) => s,
        other => panic!("parsed {other:?}"),
    }
}

// docs/specs/painless-script-score AC-1: one test, because the switch is
// process-wide and tests of the library run in parallel.
#[test]
fn painless_is_served_only_while_the_switch_is_on() {
    use super::function_score_script::{Lang, Param};
    let painless = r#"{"functions":[{"script_score":{"script":{"source":"params.a + params.s.length()","params":{"a":2,"b":3000000000,"c":1.5,"s":"x","t":true}}}}]}"#;
    let refusal = "[script_score] runs only lang [expression] on this node";
    assert!(!crate::painless_switch::native_compile());
    assert_eq!(reason(painless), refusal);
    assert_eq!(
        reason(r#"{"functions":[{"script_score":{"script":{"lang":"painless","source":"1"}}}]}"#),
        refusal
    );
    // An expression takes numbers alone, on or off.
    let text = r#"{"functions":[{"script_score":{"script":{"lang":"expression","source":"1","params":{"s":"x"}}}}]}"#;
    assert!(reason(text).contains("a param that is not a number"));
    crate::painless_switch::set_native_compile(true);
    let s = script_of(painless);
    assert_eq!(s.lang, Lang::Painless);
    assert_eq!(
        s.params,
        [
            ("a".to_owned(), Param::Int(2)),
            ("b".to_owned(), Param::Long(3_000_000_000)),
            ("c".to_owned(), Param::Double(1.5)),
            ("s".to_owned(), Param::Str("x".to_owned())),
            ("t".to_owned(), Param::Bool(true)),
        ]
    );
    let s = script_of(
        r#"{"functions":[{"script_score":{"script":{"lang":"expression","source":"1","params":{"n":4}}}}]}"#,
    );
    assert_eq!(s.lang, Lang::Expression);
    assert_eq!(
        reason(r#"{"functions":[{"script_score":{"script":{"lang":"groovy","source":"1"}}}]}"#),
        refusal
    );
    crate::painless_switch::set_native_compile(false);
    assert!(!crate::painless_switch::native_compile());
}
