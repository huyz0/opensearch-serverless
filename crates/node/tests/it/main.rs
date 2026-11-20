//! `_search` on a reader node over the compat indexes the Java shell wrote.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "../support/mod.rs"]
mod support;

mod aggs_budgets;
mod budgets;
mod built;
#[cfg(feature = "coordinator")]
mod coordinate;
mod count;
mod field_caps;
mod frozen_search;
mod open_prefetch;
mod search;
mod shards;
