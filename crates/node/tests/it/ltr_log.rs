//! docs/specs/ltr slice 4: `ext.ltr_log`, over the same fake resolver as
//! the rescore tests.

use std::sync::{Arc, Once};

use crate::ltr_rescore::Fake;
use crate::support::{request, start};
use oss_reader::ltr::models::install;

fn search(body: &str) -> (u16, String) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| install(Arc::new(Fake)));
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn logged(spec: &str) -> String {
    format!(
        r#"{{"size":10,"query":{{"bool":{{"filter":[{{"match_all":{{}}}},{{"sltr":{{"_name":"logged","featureset":"fs","params":{{"w":3}}}}}}]}}}},"ext":{{"ltr_log":{{"log_specs":{spec}}}}}}}"#
    )
}

fn count(body: &str, what: &str) -> usize {
    body.matches(what).count()
}

// AC-11
#[test]
fn each_hit_carries_the_feature_values_in_set_order() {
    let (status, body) = search(&logged(r#"{"name":"log1","named_query":"logged"}"#));
    assert_eq!(status, 200, "{body}");
    assert_eq!(count(&body, r#""_ltrlog":[{"log1":["#), 5, "{body}");
    // the three animal documents match the first feature, the food one the second
    assert_eq!(
        count(&body, r#"{"name":"animal","value":5.0}"#),
        3,
        "{body}"
    );
    assert_eq!(count(&body, r#"{"name":"food","value":3.0}"#), 1, "{body}");
    // a feature that did not match has its name only
    assert_eq!(count(&body, r#"{"name":"animal"}"#), 2, "{body}");
    assert_eq!(count(&body, r#"{"name":"food"}"#), 4, "{body}");
    // the logging query adds nothing to a score
    assert_eq!(count(&body, r#""_score":0.0"#), 5, "{body}");
}

#[test]
fn missing_as_zero_logs_zero_for_a_feature_that_did_not_match() {
    let (status, body) = search(&logged(
        r#"[{"name":"log1","named_query":"logged","missing_as_zero":true}]"#,
    ));
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        count(&body, r#"{"name":"animal","value":0.0}"#),
        2,
        "{body}"
    );
    assert_eq!(count(&body, r#"{"name":"food","value":0.0}"#), 4, "{body}");
}

#[test]
fn a_rescore_index_logs_the_models_features() {
    let body = r#"{"size":10,"query":{"match_all":{}},"rescore":{"query":{"rescore_query":{"sltr":{"model":"lin","params":{"w":3}}}}},"ext":{"ltr_log":{"log_specs":{"name":"r","rescore_index":0}}}}"#;
    let (status, body) = search(body);
    assert_eq!(status, 200, "{body}");
    assert_eq!(count(&body, r#""_ltrlog":[{"r":["#), 5, "{body}");
    assert_eq!(
        count(&body, r#"{"name":"animal","value":5.0}"#),
        3,
        "{body}"
    );
}

#[test]
fn what_cannot_be_logged_is_refused() {
    for (spec, status, reason) in [
        (
            r#"{"name":"l","named_query":"zzz"}"#,
            400,
            "No query named [zzz] found",
        ),
        (
            r#"{"name":"l","rescore_index":3}"#,
            400,
            "No rescore at [rescore_index] [3]",
        ),
        (
            r#"{"name":"l"}"#,
            400,
            "one of [named_query] or [rescore_index]",
        ),
        (r#"{"named_query":"logged"}"#, 400, "needs a [name]"),
    ] {
        let (got, body) = search(&logged(spec));
        assert_eq!(got, status, "{spec}: {body}");
        assert!(body.contains(reason), "{spec}: {body}");
    }
    let unknown = logged(r#"{"name":"l","named_query":"logged"}"#)
        .replace(r#""featureset":"fs""#, r#""featureset":"nope""#);
    let (got, body) = search(&unknown);
    assert_eq!(got, 404, "{body}");
    let other = r#"{"query":{"match_all":{}},"ext":{"rerank":{"a":1}}}"#;
    let (got, body) = search(other);
    assert_eq!(got, 400, "{body}");
}

#[test]
fn a_featureset_sltr_is_no_rescore_and_a_model_sltr_no_plain_query() {
    let rescore = r#"{"query":{"match_all":{}},"rescore":{"query":{"rescore_query":{"sltr":{"featureset":"fs","params":{"w":1}}}}}}"#;
    let (got, body) = search(rescore);
    assert_eq!(got, 400, "{body}");
    assert!(body.contains("needs a [model]"), "{body}");
}
