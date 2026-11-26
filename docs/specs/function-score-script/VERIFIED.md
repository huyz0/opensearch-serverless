# `function_score` script_score in the expression language: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- function_score_script`.

- AC-1: `an_expression_scores_like_the_same_arithmetic` in crates/node/tests/it/function_score_script.rs
- AC-2: `a_script_takes_a_filter_and_a_weight` in crates/node/tests/it/function_score_script.rs
- AC-3: `what_is_not_served_is_refused` in crates/node/tests/it/function_score_script.rs
- AC-4: `a_script_adds_no_store_request` in crates/node/tests/it/function_score_script.rs

Not run: scores against a recorded Java OpenSearch (AC-1 compares with `field_value_factor`, which the compat replay holds against Java), and a negative script result.
