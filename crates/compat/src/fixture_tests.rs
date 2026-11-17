use serde_json::json;

use super::*;

fn minimal() -> Value {
    json!({
        "name": "create index",
        "recorded_from": {"distribution": "opensearch", "version": "3.10.0", "build_hash": "abc"},
        "cleanup": [{"method": "DELETE", "path": "/compat-a", "query": {"ignore_unavailable": "true"}}],
        "steps": [{
            "name": "create",
            "request": {"method": "PUT", "path": "/compat-a", "body": {"settings": {"number_of_shards": 1}}},
            "response": {
                "status": 200,
                "headers": {"Content-Type": "application/json; charset=UTF-8"},
                "body": {"acknowledged": true}
            },
            "normalise": [{"pointer": "/body/index", "reason": "index name echoes the request"}]
        }]
    })
}

fn refusal(doc: Value) -> FixtureError {
    Fixture::from_value(&doc).unwrap_err()
}

/// Sets the member at `pointer`, creating it in its parent object if absent.
fn with(mut doc: Value, pointer: &str, value: Value) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    doc.pointer_mut(parent)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(key.to_owned(), value);
    doc
}

#[test]
fn a_complete_fixture_parses() {
    let f = Fixture::from_value(&minimal()).unwrap();
    assert_eq!(f.name, "create index");
    assert_eq!(
        f.recorded_from,
        Some(RecordedFrom {
            version: "3.10.0".into(),
            build_hash: "abc".into()
        })
    );
    assert_eq!(f.cleanup.len(), 1);
    assert_eq!(f.cleanup[0].method, Method::Delete);
    assert_eq!(f.cleanup[0].query["ignore_unavailable"], "true");
    let step = &f.steps[0];
    assert_eq!(step.request.method, Method::Put);
    assert_eq!(
        step.request.body,
        Body::Json(json!({"settings": {"number_of_shards": 1}}))
    );
    let response = step.response.as_ref().unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers["content-type"], "application/json; charset=UTF-8",
        "header names are lower-cased"
    );
    assert_eq!(step.normalise[0].pointer.to_string(), "/body/index");
    assert_eq!(step.normalise[0].reason, "index name echoes the request");
}

#[test]
fn an_unrecorded_fixture_parses_with_no_response() {
    let mut doc = minimal();
    doc.as_object_mut().unwrap().remove("recorded_from");
    doc["steps"][0].as_object_mut().unwrap().remove("response");
    let f = Fixture::from_value(&doc).unwrap();
    assert_eq!(f.recorded_from, None);
    assert_eq!(f.steps[0].response, None);
}

#[test]
fn an_ndjson_body_parses_and_encodes_one_line_per_document() {
    let doc = with(
        minimal(),
        "/steps/0/request",
        json!({"method": "POST", "path": "/_bulk", "ndjson": [{"index": {"_index": "a"}}, {"f": 1}]}),
    );
    let f = Fixture::from_value(&doc).unwrap();
    let request = &f.steps[0].request;
    assert_eq!(request.content_type(), Some("application/x-ndjson"));
    assert_eq!(
        request.encode_body().unwrap(),
        b"{\"index\":{\"_index\":\"a\"}}\n{\"f\":1}\n".to_vec()
    );
}

#[test]
fn json_and_empty_bodies_encode_as_sent() {
    let f = Fixture::from_value(&minimal()).unwrap();
    assert_eq!(f.steps[0].request.content_type(), Some("application/json"));
    assert_eq!(
        f.steps[0].request.encode_body().unwrap(),
        br#"{"settings":{"number_of_shards":1}}"#.to_vec()
    );
    assert_eq!(f.cleanup[0].content_type(), None);
    assert_eq!(f.cleanup[0].encode_body(), None);
}

#[test]
fn every_method_round_trips_its_name() {
    for name in ["GET", "PUT", "POST", "DELETE", "HEAD"] {
        assert_eq!(Method::parse(name).unwrap().as_str(), name);
    }
}

#[test]
fn a_rule_without_a_reason_is_refused() {
    let e = refusal(with(minimal(), "/steps/0/normalise/0/reason", json!("  ")));
    assert_eq!(e.at, "steps[0].normalise[0].reason");
    assert!(e.message.contains("empty"));
    let mut doc = minimal();
    doc["steps"][0]["normalise"][0]
        .as_object_mut()
        .unwrap()
        .remove("reason");
    let e = refusal(doc);
    assert_eq!(e.at, "steps[0].normalise[0]");
    assert!(e.message.contains("\"reason\""));
}

#[test]
fn a_bad_pointer_is_refused_where_it_stands() {
    let e = refusal(with(
        minimal(),
        "/steps/0/normalise/0/pointer",
        json!("body/took"),
    ));
    assert_eq!(e.at, "steps[0].normalise[0].pointer");
}

#[test]
fn an_unknown_method_is_refused() {
    let e = refusal(with(minimal(), "/steps/0/request/method", json!("PATCH")));
    assert_eq!(e.at, "steps[0].request.method");
    assert!(e.message.contains("PATCH"));
}

#[test]
fn a_request_with_two_bodies_is_refused() {
    let e = refusal(with(minimal(), "/steps/0/request/ndjson", json!([{}])));
    assert_eq!(e.at, "steps[0].request");
    assert!(e.message.contains("both"));
}

#[test]
fn an_unknown_key_is_refused_so_a_typo_cannot_drop_rules() {
    let mut doc = minimal();
    let step = doc["steps"][0].as_object_mut().unwrap();
    let rules = step.remove("normalise").unwrap();
    step.insert("normalize".into(), rules);
    let e = refusal(doc);
    assert_eq!(e.at, "steps[0]");
    assert!(e.message.contains("normalize"));
}

#[test]
fn paths_must_be_absolute_without_a_query_string() {
    for path in ["compat-a", "/compat-a?pretty"] {
        let e = refusal(with(minimal(), "/steps/0/request/path", json!(path)));
        assert_eq!(e.at, "steps[0].request.path");
    }
}

#[test]
fn statuses_outside_http_range_are_refused() {
    for status in [json!(99), json!(600), json!(70000), json!("200")] {
        let e = refusal(with(minimal(), "/steps/0/response/status", status));
        assert_eq!(e.at, "steps[0].response.status");
    }
}

#[test]
fn shapes_and_types_are_checked() {
    assert_eq!(refusal(json!([])).at, "$");
    assert_eq!(refusal(with(minimal(), "/steps", json!([]))).at, "steps");
    assert_eq!(refusal(with(minimal(), "/steps", json!({}))).at, "steps");
    assert_eq!(refusal(with(minimal(), "/cleanup", json!(1))).at, "cleanup");
    assert_eq!(refusal(with(minimal(), "/name", json!(1))).at, "name");
    assert_eq!(
        refusal(with(minimal(), "/description", json!(1))).at,
        "description"
    );
    assert_eq!(
        refusal(with(minimal(), "/steps/0/normalise", json!({}))).at,
        "steps[0].normalise"
    );
    assert_eq!(
        refusal(with(minimal(), "/steps/0/request/query", json!({"a": 1}))).at,
        "steps[0].request.query.a"
    );
    assert_eq!(
        refusal(with(minimal(), "/steps/0/request/query", json!([]))).at,
        "steps[0].request.query"
    );
    assert_eq!(
        refusal(with(minimal(), "/steps/0/request/ndjson", json!({}))).at,
        "steps[0].request"
    );
    assert_eq!(
        refusal(with(
            minimal(),
            "/steps/0/response/headers",
            json!({"X": true})
        ))
        .at,
        "steps[0].response.headers.X"
    );
    assert_eq!(
        refusal(with(minimal(), "/recorded_from/version", json!(""))).at,
        "recorded_from.version"
    );
    assert_eq!(
        refusal(with(minimal(), "/recorded_from/distribution", json!(3))).at,
        "recorded_from.distribution"
    );
}

#[test]
fn ndjson_without_body_must_be_an_array() {
    let doc = with(
        minimal(),
        "/steps/0/request",
        json!({"method": "POST", "path": "/_bulk", "ndjson": {"a": 1}}),
    );
    assert_eq!(refusal(doc).at, "steps[0].request.ndjson");
}

#[test]
fn malformed_json_text_is_refused() {
    let e = Fixture::from_slice(b"{\"name\": ").unwrap_err();
    assert_eq!(e.at, "$");
    assert!(e.to_string().starts_with("$: "));
    let ok = Fixture::from_slice(minimal().to_string().as_bytes()).unwrap();
    assert_eq!(ok.name, "create index");
}

#[test]
fn the_response_document_selects_headers_when_asked() {
    let response = Response {
        status: 201,
        headers: [("a", "1"), ("b", "2")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        body: None,
    };
    assert_eq!(
        response.to_document(None),
        json!({"status": 201, "headers": {"a": "1", "b": "2"}, "body": null})
    );
    let select: BTreeMap<String, String> = [("b".to_owned(), String::new())].into();
    assert_eq!(
        response.to_document(Some(&select)),
        json!({"status": 201, "headers": {"b": "2"}, "body": null})
    );
}

#[test]
fn a_raw_body_is_sent_as_written() {
    let doc = with(
        minimal(),
        "/steps/0/request",
        json!({"method": "PUT", "path": "/a/_doc/1", "raw": "{\"x\":1,\"x\":2}"}),
    );
    let f = Fixture::from_value(&doc).unwrap();
    let request = &f.steps[0].request;
    assert_eq!(request.content_type(), Some("application/json"));
    assert_eq!(
        request.encode_body().unwrap(),
        b"{\"x\":1,\"x\":2}".to_vec()
    );
}

#[test]
fn a_raw_body_must_be_a_string_and_stand_alone() {
    let not_text = with(
        minimal(),
        "/steps/0/request",
        json!({"method": "PUT", "path": "/a/_doc/1", "raw": {"x": 1}}),
    );
    assert_eq!(refusal(not_text).at, "steps[0].request.raw");
    let both = with(
        minimal(),
        "/steps/0/request",
        json!({"method": "PUT", "path": "/a/_doc/1", "raw": "{}", "body": {}}),
    );
    assert_eq!(refusal(both).at, "steps[0].request");
}
