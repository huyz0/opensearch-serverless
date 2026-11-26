# Verified: `_validate/query`

- AC-1: `a_query_the_search_accepts_is_valid` and `a_two_shard_index_validates_against_one_shard` in crates/node/tests/it/validate_query.rs
- AC-2: `no_body_an_empty_body_and_match_all_are_valid_by_either_method` in crates/node/tests/it/validate_query.rs
- AC-3: `a_body_that_does_not_parse_is_not_valid` in crates/node/tests/it/validate_query.rs
- AC-4: `a_query_that_fails_to_build_is_not_valid` and `a_query_this_node_does_not_support_is_refused_not_invalid` in crates/node/tests/it/validate_query.rs
- AC-5: `an_unknown_index_is_not_found_as_in_a_search` in crates/node/tests/it/validate_query.rs
- AC-6: `no_index_and_any_parameter_are_refused` in crates/node/tests/it/validate_query.rs
- AC-7: `validating_an_open_index_makes_no_request_to_the_store` in crates/node/tests/it/validate_query.rs
- AC-8: `the_q_parameters_are_validated_as_a_query_string` in crates/node/tests/it/validate_query.rs (seen red with the parameters refused, green with them served)
