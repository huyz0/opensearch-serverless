# Verified: `profile: false` and `stats` in a search body

- AC-1: `profile_false_and_stats_are_read_and_change_nothing` in crates/reader/src/dsl/source_tests.rs and `a_body_profile_false_and_stats_change_nothing` in crates/node/tests/it/search_body_keys.rs
- AC-2: `profile_true_and_a_stats_element_that_is_not_a_string_are_refused` in crates/reader/src/dsl/source_tests.rs and `a_body_profile_false_and_stats_change_nothing` in crates/node/tests/it/search_body_keys.rs
