//! The training request's own checks, against the stock 3.10.0 recording in
//! `fixtures/stock-plugins/knn-train.json`: every `_train` step whose answer
//! the body alone decides.

use serde_json::Value;

use super::{TrainRequest, parse};

fn steps() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/stock-plugins/knn-train.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc["steps"].as_array().unwrap().clone()
}

/// The steps whose refusal needs the index or the model store, not the body alone.
const NEEDS_MORE: &[&str] = &[
    "train missing index",
    "train missing field",
    "train wrong dimension",
    "train same id again",
];

fn is_train(step: &Value) -> bool {
    step["request"]["path"]
        .as_str()
        .is_some_and(|p| p.ends_with("/_train"))
}

#[test]
fn every_refusal_the_body_decides_is_the_recorded_one() {
    let mut checked = 0;
    for step in steps().iter().filter(|s| is_train(s)) {
        let name = step["name"].as_str().unwrap();
        if NEEDS_MORE.contains(&name) || step["response"]["status"] != 400 {
            continue;
        }
        let err = parse(&step["request"]["body"]).expect_err(name);
        let recorded = &step["response"]["body"]["error"];
        assert_eq!(err.status(), 400, "{name}");
        let e = err.as_exception().expect(name);
        assert_eq!(e.kind(), recorded["type"].as_str().unwrap(), "{name}");
        assert_eq!(e.reason(), recorded["reason"].as_str().unwrap(), "{name}");
        checked += 1;
    }
    assert_eq!(checked, 8);
}

#[test]
fn every_body_the_stock_plugin_accepted_is_accepted() {
    let mut checked = 0;
    for step in steps().iter().filter(|s| is_train(s)) {
        if step["response"]["status"] != 200 {
            continue;
        }
        let name = step["name"].as_str().unwrap();
        parse(&step["request"]["body"]).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        checked += 1;
    }
    assert!(checked >= 7, "{checked}");
}

#[test]
fn an_accepted_body_keeps_what_it_set_and_defaults_the_rest() {
    let body: Value = serde_json::from_str(
        r#"{"training_index":"i","training_field":"v","dimension":3,
        "method":{"name":"ivf","engine":"faiss","space_type":"innerproduct",
        "parameters":{"nlist":2,"encoder":{"name":"pq","parameters":{"m":3,"code_size":8}}}},
        "max_training_vector_count":100,"search_size":50,"description":"d"}"#,
    )
    .unwrap();
    let r: TrainRequest = parse(&body).unwrap();
    assert_eq!(
        (r.nlist, r.nprobes, r.space.as_str()),
        (2, 1, "innerproduct")
    );
    assert_eq!(r.pq.map(|p| (p.m, p.code_size)), Some((3, 8)));
    assert_eq!((r.max_training_vectors, r.search_size), (100, 50));
    assert_eq!(r.description, "d");
}
