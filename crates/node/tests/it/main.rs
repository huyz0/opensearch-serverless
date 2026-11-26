//! `_search` on a reader node over the compat indexes the Java shell wrote.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "../support/mod.rs"]
mod support;

mod aggs_budgets;
mod analyze;
mod budgets;
mod built;
mod collapse;
#[cfg(feature = "coordinator")]
mod coordinate;
mod count;
mod field_caps;
mod frozen_search;
mod function_score_random;
mod function_score_script;
mod msearch;
mod msearch_template;
mod never_written;
mod open_prefetch;
mod open_prefetch_layout;
mod rank_eval;
mod rescore;
mod script_fields;
mod script_sort;
mod search;
mod search_hits;
mod search_pipeline;
mod search_template;
mod search_template_stored;
mod shards;
mod uri_query;
mod validate_query;
