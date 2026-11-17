//! Replays the mapping steps of `fixtures/compat/mapping/` (recorded from
//! stock OpenSearch 3.10) through the writer's mapping: a create-index's
//! `mappings`, `PUT _mapping` and `GET _mapping`. A created index's mapping
//! must be written back byte for byte as OpenSearch shows it, and a refused
//! one must be refused with the same exception types and messages.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oss_writer::mapping::json::{self, Json};
use oss_writer::mapping::{Mapping, MappingError};

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat/mapping");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    out.sort();
    assert!(out.len() >= 7, "fixtures in {}", dir.display());
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
        // Once a document's dynamic fields would change the mapping, the
        // rest of the fixture waits for dynamic mapping.
        let mut grown = false;
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
                    let got = match mappings {
                        None => Ok(Mapping::default()),
                        Some(m) => Mapping::parse(m.to_json().as_bytes()),
                    };
                    expect_outcome(&step, got.as_ref().map(|_| ()).map_err(Clone::clone));
                    if let Ok(m) = got {
                        indices.insert(index.to_string(), m);
                    }
                }
                ("PUT", [index, "_mapping"]) => {
                    let current = indices.get(*index).expect("index exists").clone();
                    let got = Mapping::parse(step.body.unwrap().to_json().as_bytes())
                        .and_then(|u| current.merge(&u));
                    expect_outcome(&step, got.as_ref().map(|_| ()).map_err(Clone::clone));
                    if let Ok(m) = got {
                        indices.insert(index.to_string(), m);
                    }
                }
                ("GET", [index, "_mapping"]) if !grown => {
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
                    assert_eq!(
                        &Mapping::parse(m.to_json().as_bytes()).unwrap(),
                        m,
                        "{file} / {}",
                        step.name
                    );
                }
                ("PUT", [_, "_doc", _]) => grown = true,
                ("GET", _) => {}
                other => panic!("{file}: no replay for {other:?}"),
            }
            replayed += 1;
        }
    }
    assert!(replayed >= 40, "replayed {replayed}");
}
