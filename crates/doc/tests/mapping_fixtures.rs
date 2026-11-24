//! Replays the mapping steps of `fixtures/compat/mapping/` (recorded from
//! stock OpenSearch 3.10) through the writer's mapping: a create-index's
//! `mappings`, `PUT _mapping`, documents (`PUT _doc`) and `GET _mapping`. A
//! mapping, as created, updated or grown by dynamic mapping, must be
//! written back byte for byte as OpenSearch shows it, and a refused mapping
//! or document must be refused with the same exception types and messages.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oss_doc::document;
use oss_doc::mapping::json::{self, Json};
use oss_doc::mapping::{Mapping, MappingError, ParseOptions};

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat");
    let mut out = Vec::new();
    // The k-NN documents are replayed here too: their reads (`GET _doc`,
    // `_count`) are the reader's, and are skipped below.
    for dir in ["mapping", "knn"] {
        let mut found: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        found.sort();
        out.extend(found);
    }
    assert!(out.len() >= 9, "fixtures in {}", root.display());
    out
}

/// The error a recorded body holds, as (type, reason) pairs down its
/// `caused_by` chain.
fn recorded_error(body: &Json) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut e = body.get("error");
    while let Some(err) = e {
        let text = |k| match err.get(k) {
            Some(Json::Str(s)) => s.clone(),
            other => panic!("{k}: {other:?}"),
        };
        out.push((text("type"), text("reason")));
        e = err.get("caused_by");
    }
    out
}

fn chain(e: &MappingError) -> Vec<(String, String)> {
    let mut out = vec![(e.ty.to_string(), e.reason.clone())];
    let mut c = e.caused_by.as_deref();
    while let Some(err) = c {
        out.push((err.ty.to_string(), err.reason.clone()));
        c = err.caused_by.as_deref();
    }
    out
}

struct Step<'a> {
    file: String,
    name: &'a str,
    method: &'a str,
    path: &'a str,
    body: Option<&'a Json>,
    status: i64,
    response: Option<&'a Json>,
}

fn text<'a>(j: &'a Json, k: &str) -> &'a str {
    match j.get(k) {
        Some(Json::Str(s)) => s,
        other => panic!("{k}: {other:?}"),
    }
}

fn expect_outcome(step: &Step<'_>, got: Result<(), MappingError>) {
    match got {
        Ok(()) => assert!(
            step.status < 300,
            "{} / {}: accepted, recorded {}",
            step.file,
            step.name,
            step.status
        ),
        Err(e) => {
            assert!(
                step.status >= 400,
                "{} / {}: refused with {e:?}, recorded {}",
                step.file,
                step.name,
                step.status
            );
            assert!(!e.is_unsupported(), "{} / {}: {e}", step.file, step.name);
            assert_eq!(
                chain(&e),
                recorded_error(step.response.unwrap()),
                "{} / {}",
                step.file,
                step.name
            );
        }
    }
}

#[test]
fn mapping_steps_replay() {
    let mut replayed = 0;
    for path in fixtures() {
        let raw = std::fs::read(&path).unwrap();
        let fixture = json::parse(&raw).unwrap();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut indices: BTreeMap<String, Mapping> = BTreeMap::new();
        // Each index's `index.knn`, which the k-NN plugin's parser reads.
        let mut knn: BTreeMap<String, bool> = BTreeMap::new();
        let Some(Json::Arr(steps)) = fixture.get("steps") else {
            panic!("{file}: steps")
        };
        for s in steps {
            let request = s.get("request").unwrap();
            let response = s.get("response").unwrap();
            let step = Step {
                file: file.clone(),
                name: text(s, "name"),
                method: text(request, "method"),
                path: text(request, "path"),
                body: request.get("body"),
                status: match response.get("status") {
                    Some(Json::Num(n)) => n.parse().unwrap(),
                    other => panic!("status {other:?}"),
                },
                response: response.get("body"),
            };
            let parts: Vec<&str> = step.path.trim_start_matches('/').split('/').collect();
            match (step.method, parts.as_slice()) {
                ("PUT", [index]) => {
                    let mappings = step.body.and_then(|b| b.get("mappings"));
                    let flag = step
                        .body
                        .and_then(|b| b.get("settings"))
                        .and_then(|s| s.get("index.knn"))
                        .is_some_and(|k| matches!(k, Json::Bool(true)));
                    let options = ParseOptions { knn: Some(flag) };
                    let got = match mappings {
                        None => Ok(Mapping::default()),
                        Some(m) => Mapping::parse_with(m.to_json().as_bytes(), options),
                    };
                    knn.insert(index.to_string(), flag);
                    expect_outcome(&step, got.as_ref().map(|_| ()).map_err(Clone::clone));
                    if let Ok(m) = got {
                        indices.insert(index.to_string(), m);
                    }
                }
                ("PUT", [index, "_mapping"]) => {
                    let current = indices.get(*index).expect("index exists").clone();
                    let options = ParseOptions {
                        knn: Some(knn.get(*index).copied().unwrap_or(false)),
                    };
                    let got = Mapping::parse_with(step.body.unwrap().to_json().as_bytes(), options)
                        .and_then(|u| current.merge(&u));
                    expect_outcome(&step, got.as_ref().map(|_| ()).map_err(Clone::clone));
                    if let Ok(m) = got {
                        indices.insert(index.to_string(), m);
                    }
                }
                ("GET", [index, "_mapping"]) => {
                    let Some(m) = indices.get(*index) else {
                        assert_eq!(step.status, 404, "{file} / {}", step.name);
                        continue;
                    };
                    let recorded = step
                        .response
                        .unwrap()
                        .get(index)
                        .unwrap()
                        .get("mappings")
                        .unwrap();
                    assert_eq!(m.to_json(), recorded.to_json(), "{file} / {}", step.name);
                    // What it wrote reads back to the same mapping.
                    let options = ParseOptions {
                        knn: Some(knn.get(*index).copied().unwrap_or(false)),
                    };
                    assert_eq!(
                        &Mapping::parse_with(m.to_json().as_bytes(), options).unwrap(),
                        m,
                        "{file} / {}",
                        step.name
                    );
                }
                ("PUT", [index, "_doc", id]) => {
                    let current = indices.get(*index).expect("index exists").clone();
                    let source = match request.get("raw") {
                        Some(Json::Str(raw)) => raw.clone(),
                        _ => step.body.unwrap().to_json(),
                    };
                    let got = document::parse(&current, id, source.as_bytes());
                    expect_outcome(&step, got.as_ref().map(|_| ()).map_err(Clone::clone));
                    if let Ok(doc) = got
                        && let Some(m) = doc.grown
                    {
                        indices.insert(index.to_string(), m);
                    }
                }
                ("GET", [_, "_doc", _] | [_, "_count"]) => continue,
                other => panic!("{file}: no replay for {other:?}"),
            }
            replayed += 1;
        }
    }
    assert!(replayed >= 150, "replayed {replayed}");
}
