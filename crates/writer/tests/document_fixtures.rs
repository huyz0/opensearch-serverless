//! Replays the index, create and delete steps of
//! `fixtures/compat/documents/*.json` through a shard: every step's status,
//! `result`, `_version`, `_seq_no` and `_primary_term`, or its refusal's
//! type and message, must be the recorded one. Each fixture replays twice:
//! with every version in the version map, and with a refresh after every
//! step, so versions come from the segments.

// A fixture is a small file the test reads whole.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_writer::index::{
    Expect, Outcome, Refused, Shard, UNASSIGNED_SEQ_NO, VersionType, validate,
};
use oss_writer::mapping::Mapping;
use serde_json::{Value, json};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat/documents");
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    all.sort();
    all
}

/// `RestUtils.decodeComponent` for the ids the fixtures use.
fn decode(text: &str) -> String {
    text.replace("%20", " ")
}

/// The concurrency parameters of a request's query string.
fn expect(query: &Value) -> Expect {
    let get = |k: &str| query.get(k).and_then(Value::as_str);
    let num = |k: &str| get(k).map(|v| v.parse::<i64>().expect("number"));
    Expect {
        version: num("version").unwrap_or(oss_writer::index::MATCH_ANY),
        version_type: get("version_type").map_or(VersionType::Internal, |t| {
            VersionType::parse(t).expect("version type")
        }),
        if_seq_no: num("if_seq_no").unwrap_or(UNASSIGNED_SEQ_NO),
        if_primary_term: num("if_primary_term").unwrap_or(0),
    }
}

/// What the shard answers to one request, in the response's terms.
fn answer(
    shard: &mut Shard<'_>,
    method: &str,
    path: &str,
    query: &Value,
    body: Option<&Value>,
) -> Value {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (endpoint, id) = (parts[1], decode(parts[2]));
    let create =
        endpoint == "_create" || query.get("op_type").and_then(Value::as_str) == Some("create");
    let expect = expect(query);
    if let Err(reason) = validate(&expect, create) {
        return json!({"status": 400, "type": "action_request_validation_exception", "reason": reason});
    }
    let result = match method {
        "DELETE" => shard.delete(&id, &expect),
        _ => {
            let source = body.expect("body").to_string();
            shard.index(&id, source.as_bytes(), create, &expect)
        }
    };
    match result {
        Ok(w) => json!({
            "status": match w.outcome {
                Outcome::Created => 201,
                Outcome::NotFound => 404,
                Outcome::Updated | Outcome::Deleted => 200,
            },
            "result": w.outcome.name(),
            "_version": w.version,
            "_seq_no": w.seq_no,
            "_primary_term": w.primary_term,
        }),
        Err(Refused::Conflict(reason)) => {
            json!({"status": 409, "type": "version_conflict_engine_exception", "reason": reason})
        }
        Err(Refused::Mapping(e)) => panic!("{path}: {e:?}"),
    }
}

/// The recorded response in [`answer`]'s terms.
fn recorded(response: &Value) -> Value {
    let body = &response["body"];
    let status = response["status"].clone();
    match body.get("error") {
        Some(e) => json!({"status": status, "type": e["type"], "reason": e["reason"]}),
        None => json!({
            "status": status,
            "result": body["result"],
            "_version": body["_version"],
            "_seq_no": body["_seq_no"],
            "_primary_term": body["_primary_term"],
        }),
    }
}

fn replay(path: &PathBuf, refresh_each: bool) {
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json");
    let steps = fixture["steps"].as_array().expect("steps");
    let create = &steps[0]["request"];
    let mapping = match create["body"].get("mappings") {
        Some(m) => Mapping::parse(m.to_string().as_bytes()).expect("mapping"),
        None => Mapping::default(),
    };
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");
    for step in &steps[1..] {
        let req = &step["request"];
        let method = req["method"].as_str().expect("method");
        let p = req["path"].as_str().expect("path");
        let query = req.get("query").cloned().unwrap_or(json!({}));
        let got = answer(&mut shard, method, p, &query, req.get("body"));
        assert_eq!(
            got,
            recorded(&step["response"]),
            "{}: {} (refresh each: {refresh_each})",
            path.display(),
            step["name"]
        );
        if refresh_each {
            shard.refresh().expect("refresh");
        }
    }
}

#[test]
fn document_steps_answer_as_the_java_does() {
    let all = fixtures();
    assert!(!all.is_empty());
    for path in &all {
        replay(path, false);
        replay(path, true);
    }
}
