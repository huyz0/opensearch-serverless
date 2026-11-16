//! Document routing (docs/specs/routing/SPEC.md), checked against what the
//! Java shell's own routing classes and core's `OperationRouting` answer
//! (`fixtures/routing/`, recorded by `Routings.java`).

#![allow(clippy::disallowed_methods)]
// ARITH: indexes into fixture rows and small test counts; an overflow
// panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

mod hashes;
mod layouts;

use std::path::PathBuf;

use oss_cluster::routing::Unit;

/// The tab-separated rows of fixture `name`.
pub(crate) fn rows(name: &str) -> Vec<Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/routing")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

/// A string from its UTF-8 in hex, `-` for empty.
pub(crate) fn unhex(text: &str) -> String {
    if text == "-" {
        return String::new();
    }
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect();
    String::from_utf8(bytes).unwrap()
}

/// An unsigned hash from the Java's `Long.toHexString`.
pub(crate) fn hash(text: &str) -> u64 {
    u64::from_str_radix(text, 16).unwrap()
}

/// A unit from `id:first:last`.
pub(crate) fn unit(text: &str) -> Unit {
    let parts: Vec<&str> = text.split(':').collect();
    Unit {
        id: parts[0].parse().unwrap(),
        first: hash(parts[1]),
        last: hash(parts[2]),
    }
}
