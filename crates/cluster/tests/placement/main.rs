//! Placement (docs/specs/placement/SPEC.md), checked against what the Java
//! shell's own classes answer for one recorded fleet
//! (`fixtures/placement/`, recorded by `Placements.java`), and the parts
//! that are random or local (the hot-shard spread) by their properties.

#![allow(clippy::disallowed_methods)]
// ARITH: times and counts here are test constants and small sums; an
// overflow panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

mod fleet;
mod java;
mod ramp;
mod spread;

use std::path::PathBuf;

use oss_format::NodeLease;

/// The time the fleet was recorded at.
pub(crate) const NOW: i64 = 1_700_000_000_000;

/// The lease lifetime it was recorded with.
pub(crate) const TTL: i64 = 30_000;

/// The tab-separated rows of fixture `name`.
pub(crate) fn rows(name: &str) -> Vec<Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/placement")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

/// The recorded fleet: twelve leases, as the Java wrote them.
pub(crate) fn fleet() -> Vec<NodeLease> {
    rows("fleet.txt")
        .iter()
        .map(|r| NodeLease::parse(r[0].as_bytes()).unwrap())
        .collect()
}

/// A comma-separated list, empty for none.
pub(crate) fn list(field: &str) -> Vec<&str> {
    field.split(',').filter(|s| !s.is_empty()).collect()
}
