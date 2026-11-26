# Verified: `_count`

- AC-1: `a_count_equals_the_searchs_exact_total_over_one_shard` in crates/node/tests/it/count.rs
- AC-2: `a_count_equals_the_searchs_exact_total_over_two_shards` in crates/node/tests/it/count.rs
- AC-3: `no_body_an_empty_body_and_match_all_count_everything_by_either_method` in crates/node/tests/it/count.rs
- AC-4: `an_unknown_index_is_not_found_as_in_a_search` in crates/node/tests/it/count.rs
- AC-5: `a_query_the_search_refuses_is_refused_the_same_way` in crates/node/tests/it/count.rs
- AC-6: `a_body_key_other_than_query_and_an_unknown_parameter_are_refused` in crates/node/tests/it/count.rs
- AC-7: `a_count_of_an_open_index_makes_no_request_to_the_store` in crates/node/tests/it/count.rs
- AC-8: `min_score_counts_only_what_scores_at_least_it` and `min_score_counts_the_same_over_two_shards` in crates/node/tests/it/count.rs (seen red with the parameter refused, green with it served)
