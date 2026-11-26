//! Tests of the `inner_hits` parser of a nested query; compiled only under
//! `#[cfg(test)]`.

use super::inner_hits::InnerHits;
use super::*;
use crate::dsl::parse_chunks;

fn parsed(inner: &str) -> Result<InnerHits, String> {
    let body = format!(
        r#"{{"query":{{"nested":{{"path":"c","query":{{"match_all":{{}}}},"inner_hits":{inner}}}}}}}"#
    );
    match parse_chunks([body.as_bytes()]) {
        Ok(s) => match s.query {
            Some(Query::More(m)) => match *m {
                More::Nested(q, _) => Ok(q.inner_hits.expect("inner_hits kept")),
                other => panic!("parsed {other:?}"),
            },
            other => panic!("parsed {other:?}"),
        },
        Err(e) => Err(e.into_failure().reason),
    }
}

#[test]
fn an_empty_inner_hits_takes_the_defaults() {
    let h = parsed("{}").expect("parses");
    assert_eq!((h.name, h.from, h.size), (None, 0, 3));
    assert!(h.sort.is_empty() && !h.source_skip);
}

#[test]
fn inner_hits_reads_its_name_window_sort_and_source() {
    let h = parsed(r#"{"name":"kids","from":1,"size":5,"sort":[{"c.w":"desc"}],"_source":false}"#)
        .expect("parses");
    assert_eq!(h.name.as_deref(), Some("kids"));
    assert_eq!(
        (h.from, h.size, h.sort.len(), h.source_skip),
        (1, 5, 1, true)
    );
}

#[test]
fn a_negative_window_fails_as_opensearch_does() {
    assert_eq!(
        parsed(r#"{"from":-1}"#).unwrap_err(),
        "illegal from value, at least 0 or higher"
    );
    assert_eq!(
        parsed(r#"{"size":-1}"#).unwrap_err(),
        "illegal size value, at least 0 or higher"
    );
}

#[test]
fn what_inner_hits_does_not_serve_is_refused_by_name() {
    for (body, key) in [
        (r#"{"explain":true}"#, "explain"),
        (r#"{"version":true}"#, "version"),
        (r#"{"docvalue_fields":["c.w"]}"#, "docvalue_fields"),
        (r#"{"highlight":{"fields":{}}}"#, "highlight"),
        (r#"{"script_fields":{}}"#, "script_fields"),
        (r#"{"inner_hits":{}}"#, "inner_hits"),
        (r#"{"_source":["c.w"]}"#, "_source"),
    ] {
        let reason = parsed(body).unwrap_err();
        assert!(
            reason.contains(&format!("[inner_hits] [{key}]")) && reason.contains("not supported"),
            "{key}: {reason}"
        );
    }
}

#[test]
fn an_unknown_inner_hits_key_fails_by_name() {
    assert!(
        parsed(r#"{"nope":1}"#)
            .unwrap_err()
            .contains("[inner_hits] unknown field [nope]")
    );
}
