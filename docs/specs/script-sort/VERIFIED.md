# `_script` sort in the expression language: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- script_sort`.

- AC-1: `an_expression_sorts_like_the_field_it_reads` in crates/node/tests/it/script_sort.rs
- AC-2: `an_expression_sorts_like_the_field_it_reads` in crates/node/tests/it/script_sort.rs
- AC-3: `a_two_shard_index_sorts_through_the_coordinator` in crates/node/tests/it/script_sort.rs
- AC-4: `what_is_not_served_is_refused` in crates/node/tests/it/script_sort.rs
- AC-5: `a_script_sort_adds_no_store_request` in crates/node/tests/it/script_sort.rs

Not run: sort values against a recorded Java OpenSearch, and `_script` in inner hits or `top_hits` (refused in code, not asserted by a test).
