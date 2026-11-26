# `_search` collapse: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- collapse::` and `cargo test -p oss-reader --lib collapse`.

- AC-1: `a_keyword_keeps_the_best_hit_of_each_value` in crates/node/tests/it/collapse.rs
- AC-2: `from_and_size_page_the_groups` in crates/node/tests/it/collapse.rs
- AC-3: `a_sort_orders_the_groups_by_their_best_hit` in crates/node/tests/it/collapse.rs
- AC-4: `a_number_collapses_the_same_way` in crates/node/tests/it/collapse.rs
- AC-5: `what_stock_refuses_is_refused` in crates/node/tests/it/collapse.rs and `what_stock_refuses_is_refused_as_stock_does` in crates/reader/src/dsl/collapse_tests.rs
- AC-6: `the_window_grows_by_four_to_the_cap_and_no_further` in crates/reader/src/search_collapse_tests.rs
- AC-7: `a_collapse_adds_no_warm_store_request` in crates/node/tests/it/collapse.rs

Not run: a search that exceeds the cap end to end (it needs over 100,000 matching documents), a document without a value in the collapse field (the compat index has none), and answers against a recorded Java OpenSearch.
