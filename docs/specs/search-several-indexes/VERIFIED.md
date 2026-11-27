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

Not tested: the 1,024-shard bound (it needs an index set of that many shards), and a single-index list's default pipeline.
- AC-12: `a_count_over_a_list_or_an_alias_of_several_counts_them_all` and `a_count_over_a_list_with_a_missing_index_names_it_alone` in crates/node/tests/it/search_several.rs (seen red before a count took the names, green after)
