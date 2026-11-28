use serde_json::{Value, json};

use super::*;

fn def(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn trusted() -> Trusted {
    Trusted::parse(r"^http://127\.0\.0\.1:18080/.*$").unwrap()
}

fn good() -> Value {
    json!({"name": "c", "protocol": "http", "parameters": {"endpoint": "127.0.0.1:18080"},
           "actions": [{"action_type": "predict", "method": "POST",
                        "url": "http://127.0.0.1:18080/json"}]})
}

#[test]
fn a_trusted_url_passes_and_an_untrusted_one_is_refused_with_stocks_text() {
    validate(&def(good()), &trusted()).unwrap();
    let mut other = good();
    other["actions"][0]["url"] = json!("http://203.0.113.9/x");
    assert_eq!(
        validate(&def(other), &trusted()).unwrap_err(),
        "Connector URL is not matching the trusted connector endpoint regex"
    );
}

#[test]
fn no_pattern_trusts_nothing_and_a_pattern_must_match_the_whole_url() {
    assert!(validate(&def(good()), &Trusted::default()).is_err());
    assert!(validate(&def(good()), &Trusted::parse("").unwrap()).is_err());
    let prefix = Trusted::parse(r"http://127\.0\.0\.1").unwrap();
    assert!(validate(&def(good()), &prefix).is_err());
    assert!(Trusted::parse("(").is_err());
}

#[test]
fn a_url_is_matched_after_its_parameters_are_filled_in() {
    let mut c = good();
    c["actions"][0]["url"] = json!("http://${parameters.endpoint}/json");
    validate(&def(c.clone()), &trusted()).unwrap();
    c["actions"][0]["url"] = json!("http://${parameters.missing}/json");
    assert!(validate(&def(c), &trusted()).is_err());
}

#[test]
fn a_missing_name_or_protocol_and_an_unknown_protocol_are_refused() {
    let mut c = good();
    c["name"] = json!("");
    assert_eq!(
        validate(&def(c), &trusted()).unwrap_err(),
        "Connector name is null"
    );
    let mut c = good();
    c["protocol"] = json!("carrier");
    assert_eq!(
        validate(&def(c), &trusted()).unwrap_err(),
        "Unsupported connector protocol. Please use one of \
         [aws_sigv4, http, google_cloud, mcp_sse, mcp_streamable_http]"
    );
    let mut c = good();
    c.as_object_mut().unwrap().remove("protocol");
    assert!(validate(&def(c), &trusted()).is_err());
}

#[test]
fn a_malformed_action_is_refused_and_no_actions_is_fine() {
    let mut c = good();
    c["actions"] = json!([]);
    validate(&def(c), &trusted()).unwrap();
    for bad in [
        json!("x"),
        json!([1]),
        json!([{"url": "http://127.0.0.1:18080/a"}]),
        json!([{"action_type": "predict"}]),
    ] {
        let mut c = good();
        c["actions"] = bad;
        assert!(validate(&def(c), &trusted()).is_err());
    }
}

#[test]
fn sealing_encrypts_each_credential_value_and_upper_cases_action_types() {
    let key = credentials::new_master_key().unwrap();
    let mut c = def(good());
    c.insert(
        "credential".into(),
        json!({"key": "AKIA-secret", "token": "t0k"}),
    );
    seal_definition(&mut c, &key).unwrap();
    assert_eq!(c["actions"][0]["action_type"], "PREDICT");
    let sealed = c["credential"]["key"].as_str().unwrap();
    assert!(!sealed.contains("AKIA-secret"));
    assert_eq!(credentials::open(&key, sealed).unwrap(), "AKIA-secret");
    let mut c = def(good());
    c.insert("credential".into(), json!({"k": 1}));
    assert!(seal_definition(&mut c, &key).is_err());
    c.insert("credential".into(), json!("x"));
    assert!(seal_definition(&mut c, &key).is_err());
}
