//! docs/specs/pipeline-simulate: what a simulation body names and how a run answers.

use oss_coordinator::ingest::Pipeline;
use oss_coordinator::pipeline_simulate::{MAX_DOCS, read, render};

const UPPER: &[u8] = br#"{"processors":[{"uppercase":{"field":"a"}}]}"#;

fn reason(body: &str, stored: bool) -> (u16, String) {
    let refusal = read(body.as_bytes(), stored).expect_err("refused");
    (refusal.status, refusal.reason)
}

// AC-1
#[test]
fn the_body_is_read_and_its_faults_named() {
    let ok = read(
        br#"{"docs":[{"_source":{"a":"x"}},{"_index":"i","_id":"7","_source":{"a":"y"}}]}"#,
        true,
    )
    .expect("read");
    assert_eq!(ok.pipeline, None);
    assert_eq!(
        (ok.docs[0].index.as_str(), ok.docs[0].id.as_str()),
        ("_index", "_id")
    );
    assert_eq!(
        (ok.docs[1].index.as_str(), ok.docs[1].id.as_str()),
        ("i", "7")
    );
    assert_eq!(ok.docs[1].source, br#"{"a":"y"}"#);
    assert_eq!(
        reason("[1]", true).1,
        "could not parse the simulation: the body is not a JSON object"
    );
    assert_eq!(reason("{}", true).1, "[docs] required property is missing");
    assert_eq!(
        reason(r#"{"docs":[]}"#, true).1,
        "must specify at least one document in [docs]"
    );
    assert_eq!(
        reason(r#"{"docs":[1]}"#, true).1,
        "each document in [docs] must be an object"
    );
    assert_eq!(
        reason(r#"{"docs":[{}]}"#, true).1,
        "[_source] required property is missing"
    );
    assert_eq!(
        reason(r#"{"docs":[{"_source":[]}]}"#, true).1,
        "[_source] required property is missing"
    );
    assert_eq!(
        reason(r#"{"docs":[{"_id":1,"_source":{}}]}"#, true).1,
        "[_id] must be a string"
    );
    assert_eq!(
        reason(r#"{"docs":[{"_index":1,"_source":{}}]}"#, true).1,
        "[_index] must be a string"
    );
    let many = format!(
        r#"{{"docs":[{}]}}"#,
        vec![r#"{"_source":{}}"#; MAX_DOCS + 1].join(",")
    );
    assert!(reason(&many, true).1.contains("at most 100"));
    let full = format!(
        r#"{{"docs":[{}]}}"#,
        vec![r#"{"_source":{}}"#; MAX_DOCS].join(",")
    );
    assert_eq!(
        read(full.as_bytes(), true).expect("read").docs.len(),
        MAX_DOCS
    );
    // The inline form needs a pipeline; the stored form refuses one.
    assert_eq!(
        reason(r#"{"docs":[{"_source":{}}]}"#, false).1,
        "[pipeline] required property is missing"
    );
    let both = r#"{"pipeline":{"processors":[]},"docs":[{"_source":{}}]}"#;
    assert_eq!(
        reason(both, true).1,
        "[pipeline] is not allowed when the pipeline is named in the path"
    );
    let inline = format!(
        r#"{{"pipeline":{},"docs":[{{"_source":{{"a":"x"}}}}]}}"#,
        String::from_utf8_lossy(UPPER)
    );
    let read = read(inline.as_bytes(), false).expect("read");
    assert_eq!(read.pipeline.as_deref(), Some(UPPER));
}

// AC-1
#[test]
fn an_inline_pipeline_the_node_cannot_run_is_refused() {
    let body = r#"{"pipeline":{"processors":[{"grok":{"field":"a","patterns":["x"]}}]},"docs":[{"_source":{}}]}"#;
    let refusal = read(body.as_bytes(), false).expect_err("refused");
    assert_eq!((refusal.status, refusal.kind), (400, "invalid_pipeline"));
}

// AC-2
#[test]
fn each_document_is_run_and_a_failing_one_does_not_fail_the_rest() {
    let pipeline =
        Pipeline::compile(br#"{"processors":[{"uppercase":{"field":"a"}}]}"#).expect("compiles");
    let simulation = read(
        br#"{"docs":[{"_source":{"a":"x"}},{"_source":{"b":1}},{"_id":"9","_source":{"a":"q\""}}]}"#,
        true,
    )
    .expect("read");
    let answer = render(&pipeline, &simulation.docs);
    let json: serde_json::Value = serde_json::from_str(&answer).expect("json");
    let docs = json["docs"].as_array().expect("array");
    assert_eq!(docs.len(), 3);
    assert_eq!(docs[0]["doc"]["_source"]["a"], "X");
    assert_eq!(docs[0]["doc"]["_index"], "_index");
    assert_eq!(docs[1]["error"]["type"], "illegal_argument_exception");
    assert_eq!(
        docs[1]["error"]["root_cause"][0]["reason"],
        docs[1]["error"]["reason"]
    );
    assert!(
        docs[1]["error"]["reason"]
            .as_str()
            .expect("text")
            .contains('a')
    );
    assert_eq!(docs[2]["doc"]["_id"], "9");
    assert_eq!(docs[2]["doc"]["_source"]["a"], "Q\"");
}
