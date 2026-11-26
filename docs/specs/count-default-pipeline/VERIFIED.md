# `_count` default search pipeline: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- search_pipeline::`.

- AC-1: `a_count_runs_the_index_default` in crates/node/tests/it/search_pipeline.rs
- AC-2: `a_count_runs_the_index_default` in crates/node/tests/it/search_pipeline.rs
- AC-3: `a_count_ignores_a_default_when_search_pipelines_are_off` in crates/node/tests/it/search_pipeline.rs
- AC-4: `a_count_of_an_index_with_no_default_makes_no_request` in crates/node/tests/it/search_pipeline.rs

`a_count_runs_the_index_default` was seen failing (count 5 for the filtered index) with the change reverted. Not run: a sharded index's count with a default (the coordinator path reuses the query the pipeline already narrowed).
