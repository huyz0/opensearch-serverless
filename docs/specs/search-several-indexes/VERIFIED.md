# Verified: `_search` over several indexes

- AC-1: `a_list_of_indexes_is_searched_as_one` in crates/node/tests/it/search_several.rs (seen red before the change, green after)
- AC-2: `an_alias_of_several_is_the_list_and_a_repeat_is_searched_once` in crates/node/tests/it/search_several.rs
- AC-3: `a_name_that_is_no_index_refuses_the_search` in crates/node/tests/it/search_several.rs (asserts the error names the missing index alone and returns no hits, which the unchanged code, naming the whole list, does not)
- AC-4: `a_size_merges_across_the_indexes` in crates/node/tests/it/search_several.rs (the page's ids are the first three of the whole result)
- AC-5: `collapse_and_a_view_are_refused_over_several_indexes` in crates/node/tests/it/search_several.rs
- AC-6: `a_list_search_takes_a_permit_like_any_search` in crates/node/tests/it/search_several.rs (seen red with the early return before the admission, green after)
- AC-7: `a_search_may_name_at_most_the_limit_of_indexes` in crates/node/tests/it/search_several.rs
- AC-8: `routing_narrows_each_index_of_the_list` in crates/node/tests/it/search_several.rs
- AC-9: `a_list_costs_what_its_indexes_cost_alone_and_lists_nothing` in crates/node/tests/it/search_several.rs
- AC-10: `a_name_that_is_no_index_refuses_the_search` in crates/node/tests/it/search_several.rs (its second half, an alias with a missing member)
- AC-11: `one_name_answers_as_before` in crates/node/tests/it/search_several.rs

Not tested: a single-index list's default pipeline.
- AC-12: `a_count_over_a_list_or_an_alias_of_several_counts_them_all` and `a_count_over_a_list_with_a_missing_index_names_it_alone` in crates/node/tests/it/search_several.rs (seen red before a count took the names, green after)
- AC-13: `an_msearch_item_over_a_list_or_an_alias_of_several_searches_them_all` in crates/node/tests/it/search_several.rs
- AC-14: `field_caps_over_a_list_or_an_alias_of_several_names_the_indexes` in crates/node/tests/it/search_several.rs and `field_caps_of_an_alias_names_the_index_it_stands_for` (its last step, a missing member) in crates/node/tests/it/search_alias.rs. Both were red as a compile failure with the old resolution removed, not as an assertion.
- AC-15: `ignore_unavailable_skips_the_names_that_are_no_index` in crates/node/tests/it/search_several.rs (seen red while the parameter was refused as unknown, green after)
- AC-16: `ignore_unavailable_with_nothing_left_answers_an_empty_result` in crates/node/tests/it/search_several.rs (same)
- AC-17: `a_list_visiting_more_shards_than_the_limit_is_refused_before_a_shard_is_read` and `the_shard_limit_counts_the_shards_visited_not_the_ones_declared` in crates/node/tests/it/search_several_budget.rs (the first seen red with the limit raised by ten, green after; the descriptor of one index says 1,024 shards and only its shard 0 has blobs)
- AC-18: `an_msearch_item_costs_the_same_each_time_and_its_parameters_cost_nothing` and `field_caps_over_a_list_costs_the_same_each_time_and_lists_nothing` in crates/node/tests/it/search_several_budget.rs (they pin what the paths already did, so there was no red to see)
