use serde_json::{Map, Value, json};

use super::*;

fn stored(key: &str) -> Map<String, Value> {
    let mut c = json!({"name": "c", "protocol": "http",
        "parameters": {"endpoint": "127.0.0.1:18080"},
        "credential": {"api_key": "sk-secret"},
        "actions": [{"action_type": "PREDICT", "method": "POST",
            "url": "http://${parameters.endpoint}/json",
            "headers": {"authorization": "Bearer ${credential.api_key}"},
            "request_body": "{\"input\":\"${parameters.input}\"}"}]})
    .as_object()
    .unwrap()
    .clone();
    crate::connector::seal_definition(&mut c, key).unwrap();
    c
}

fn input(v: &str) -> Vec<(String, Option<String>)> {
    vec![("input".into(), Some(v.into()))]
}

#[test]
fn the_request_carries_the_parameters_and_the_opened_credential() {
    let key = credentials::new_master_key().unwrap();
    let remote = Remote::open(&stored(&key), &key).unwrap();
    let p = remote.prepare(&input("hi"), "20260101T000000Z").unwrap();
    assert_eq!(
        (p.method.as_str(), p.url.as_str()),
        ("POST", "http://127.0.0.1:18080/json")
    );
    assert_eq!(
        String::from_utf8(p.body.clone()).unwrap(),
        r#"{"input":"hi"}"#
    );
    assert!(
        p.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer sk-secret")
    );
}

#[test]
fn a_placeholder_left_unfilled_is_stocks_400() {
    let key = credentials::new_master_key().unwrap();
    let remote = Remote::open(&stored(&key), &key).unwrap();
    for params in [vec![], vec![("nope".to_owned(), Some("x".to_owned()))]] {
        let f = remote.prepare(&params, "20260101T000000Z").unwrap_err();
        assert_eq!(f.status, 400);
        assert_eq!(
            f.reason,
            "Some parameter placeholder not filled in payload: input"
        );
    }
}

#[test]
fn a_json_response_is_shown_as_the_response_map() {
    let key = credentials::new_master_key().unwrap();
    let remote = Remote::open(&stored(&key), &key).unwrap();
    let p = remote.prepare(&input("hi"), "20260101T000000Z").unwrap();
    let out = remote.read(&p, r#"{"out":[1,2,3],"echo":"ok"}"#).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v,
        json!({"inference_results": [{"output": [{"name": "response",
            "dataAsMap": {"out": [1, 2, 3], "echo": "ok"}}], "status_code": 200}]})
    );
}

#[test]
fn a_wrong_key_does_not_open_the_connector_and_says_nothing_of_the_secret() {
    let key = credentials::new_master_key().unwrap();
    let other = credentials::new_master_key().unwrap();
    let f = Remote::open(&stored(&key), &other).unwrap_err();
    assert_eq!(f.status, 500);
    assert!(!f.reason.contains("sk-secret"));
}

#[test]
fn a_connector_with_a_script_is_refused_not_run() {
    let key = credentials::new_master_key().unwrap();
    let mut c = stored(&key);
    c["actions"][0]["post_process_function"] = json!("def x = params.result;");
    let remote = Remote::open(&c, &key).unwrap();
    let p = remote.prepare(&input("hi"), "20260101T000000Z").unwrap();
    assert_eq!(remote.read(&p, r#"{"a":1}"#).unwrap_err().status, 501);
}

#[test]
fn the_signing_time_is_the_utc_date_in_sigv4_form() {
    assert_eq!(amz_date(0), "19700101T000000Z");
    assert_eq!(amz_date(1_709_210_096), "20240229T123456Z");
    assert_eq!(amz_date(4_102_444_799), "20991231T235959Z");
}
