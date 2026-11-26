# `function_score` random_score and min_score: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- function_score_random` and `cargo test -p oss-reader --lib function_score`.

- AC-1: `a_random_score_without_a_seed_is_in_the_unit_interval` in crates/node/tests/it/function_score_random.rs
- AC-2: `a_seed_and_a_field_give_the_same_scores_every_time` in crates/node/tests/it/function_score_random.rs
- AC-3: `the_score_follows_the_field_it_hashes` in crates/node/tests/it/function_score_random.rs
- AC-4: `a_seed_that_is_not_stocks_is_refused` in crates/node/tests/it/function_score_random.rs
- AC-5: `min_score_keeps_what_scores_at_least_it` in crates/node/tests/it/function_score_random.rs
- AC-6: `a_random_score_adds_no_store_request` in crates/node/tests/it/function_score_random.rs

Not run: scores against a recorded Java OpenSearch (the hash and salt follow `RandomScoreFunctionBuilder` and the engine's `RandomScoreFunction`, and the seed and salt arithmetic is unit-tested in `a_random_score_reads_its_seed_as_stock_does` and `the_seed_is_salted_with_the_shard`).
