# `_search` rescore: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- rescore::` and `cargo test -p oss-reader --lib rescore`.

- AC-1: `a_total_adds_the_rescore_query_to_what_it_matches` in crates/node/tests/it/rescore.rs
- AC-2: `the_weights_and_modes_combine_as_stock` in crates/node/tests/it/rescore.rs
- AC-3: `the_window_bounds_what_is_rescored` in crates/node/tests/it/rescore.rs
- AC-4: `rescorers_run_in_order` in crates/node/tests/it/rescore.rs
- AC-5: `what_stock_refuses_is_refused` in crates/node/tests/it/rescore.rs and `what_stock_refuses_is_refused_as_stock_does` in crates/reader/src/dsl/rescore_tests.rs
- AC-6: `a_rescore_adds_no_warm_store_request` in crates/node/tests/it/rescore.rs

Not run: scores against a recorded Java OpenSearch (the arithmetic is the engine's `opensearch_query_rescore`, which lucene-rust holds against Java), and `index.max_rescore_window` as a setting.
