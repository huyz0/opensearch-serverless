//! Zones (docs/specs/zones/SPEC.md): the zone from the awareness
//! attribute, the cross-zone byte meter, and the bytes the zone preference
//! keeps inside a zone on a modelled fleet.

#![allow(clippy::disallowed_methods)]
// ARITH: counts here are test constants and small sums; an overflow panics
// the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

mod attribute;
mod cost;
mod traffic;
