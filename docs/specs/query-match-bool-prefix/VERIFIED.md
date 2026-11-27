# Verified: the `match_bool_prefix` query

- AC-1: `the_last_term_is_a_prefix_and_the_rest_are_terms` in crates/reader/src/dsl/queries/bool_prefix_tests.rs and `the_last_term_matches_by_prefix` in crates/node/tests/it/search_match_bool_prefix.rs
- AC-2: `one_term_and_other_field_types` in crates/reader/src/dsl/queries/bool_prefix_tests.rs
- AC-3: `bad_bodies_and_unserved_options_fail` in crates/reader/src/dsl/queries/bool_prefix_tests.rs
