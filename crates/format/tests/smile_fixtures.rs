//! docs/specs/stock-snapshot-restore AC-1: the SMILE a Jackson writer wrote
//! decodes to the JSON the same writer wrote beside it.
//!
//! The documents are `fixtures/snapshots/stock-smile/` (see its README): each
//! case in three encoder settings (shared names; shared names and values;
//! and 7-bit binary).

// ARITH: test arithmetic is small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{SmileLimits, smile_to_json};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock-smile")
}

fn decode(bytes: &[u8], what: &str) -> serde_json::Value {
    let mut out = Vec::new();
    if let Err(e) = smile_to_json(bytes, &SmileLimits::default(), &mut out) {
        panic!("{what}: {e}");
    }
    serde_json::from_slice(&out).expect("valid JSON")
}

fn cases() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir())
        .expect("fixtures")
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            n.strip_suffix(".json").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// The same numbers, comparing a float by value: the writer's JSON prints a
/// float32 in its own shortest form, which reads back as a different double.
fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::{Array, Number, Object};
    match (a, b) {
        (Number(x), Number(y)) => {
            x == y
                || x.as_f64()
                    .zip(y.as_f64())
                    .is_some_and(|(x, y)| x == y || (x - y).abs() <= x.abs().max(y.abs()) * 1e-6)
        }
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Object(x), Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

#[test]
fn every_recorded_document_decodes_to_its_json() {
    let names = cases();
    assert!(names.len() >= 10, "{names:?}");
    for name in names {
        let want: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir().join(format!("{name}.json"))).unwrap())
                .expect("json");
        for mode in 0..3 {
            let smile = std::fs::read(dir().join(format!("{name}.m{mode}.smile"))).expect("smile");
            let got = decode(&smile, &format!("{name} mode {mode}"));
            assert!(same(&got, &want), "{name} mode {mode}:\n{got}\n!=\n{want}");
        }
    }
}
