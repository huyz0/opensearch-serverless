# Verified: `timeout` in the `_search` body

- AC-1: `a_body_timeout_is_read_as_a_time_value_and_changes_nothing` in crates/reader/src/dsl/source_tests.rs (seen red as a refusal, green after) and `a_body_timeout_changes_nothing_and_a_bad_one_is_refused` in crates/node/tests/it/search_body_keys.rs
- AC-2: `a_body_timeout_is_read_as_a_time_value_and_changes_nothing` in crates/reader/src/dsl/source_tests.rs and `a_body_timeout_changes_nothing_and_a_bad_one_is_refused` in crates/node/tests/it/search_body_keys.rs
