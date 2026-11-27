//! Tests of the Painless switch on `script_score` and the `script` query;
//! compiled only under `#[cfg(test)]`. One test toggles the process-wide
//! switch, because tests of the library run in parallel.

use super::function_score::{Function, FunctionScore};
use super::*;
use crate::dsl::parse_chunks;

fn parsed(body: &str) -> (FunctionScore, f32) {
    let text = format!(r#"{{"query":{{"function_score":{body}}}}}"#);
    match parse_chunks([text.as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => match *m {
            More::FunctionScore(q, boost) => (*q, boost),
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
}

fn reason(body: &str) -> String {
    let text = format!(r#"{{"query":{{"function_score":{body}}}}}"#);
    parse_chunks([text.as_bytes()])
        .expect_err("fails")
        .into_failure()
        .reason
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
    let refusal = "[script] runs only lang [expression] on this node";
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
    // The `script` query, over the same switch.
    let query = |body: &str| {
        let text = format!(r#"{{"query":{{"script":{body}}}}}"#);
        parse_chunks([text.as_bytes()])
            .map(|s| s.query)
            .map_err(|e| e.into_failure().reason)
    };
    let one =
        r#"{"script":{"source":"doc['year'].value > params.y","params":{"y":2000}},"boost":2.0}"#;
    match query(one) {
        Ok(Some(Query::More(m))) => match *m {
            More::Script(script, boost) => {
                assert_eq!(script.lang, Lang::Painless);
                assert_eq!(boost, 2.0);
            }
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
    assert!(matches!(query(r#"{"script":"true"}"#), Ok(Some(_))));
    assert!(
        query(r#"{"script":{"lang":"expression","source":"1"}}"#)
            .expect_err("refused")
            .contains("runs only lang [painless]")
    );
    assert!(
        query(r#"{"boost":1}"#)
            .expect_err("refused")
            .contains("must be provided with a [script]")
    );
    assert!(
        query(r#"{"script":"true","_name":"a"}"#)
            .expect_err("refused")
            .contains("[_name]")
    );
    assert!(
        query(r#"{"script":"true","x":1}"#)
            .expect_err("refused")
            .contains("does not support [x]")
    );
    crate::painless_switch::set_native_compile(false);
    assert!(!crate::painless_switch::native_compile());
    assert!(
        query(one)
            .expect_err("refused")
            .contains("the [script] query")
    );
}
