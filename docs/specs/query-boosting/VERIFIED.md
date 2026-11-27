# Verified: the `boosting` query

- AC-1: `boosting_holds_both_queries_and_its_factors` in crates/reader/src/dsl/queries/compound_tests.rs, `boosting_builds_a_function_score_over_the_positive_query` in crates/reader/src/dsl/queries/build_tests.rs, and `a_negative_match_scales_the_score_and_the_rest_keep_it` in crates/node/tests/it/search_boosting.rs
- AC-2: `boosting_fails_as_the_builder_fails` in crates/reader/src/dsl/queries/compound_tests.rs and `a_missing_part_is_refused_as_opensearch_refuses_it` in crates/node/tests/it/search_boosting.rs
