# Verified: `_search` over several indexes

- AC-1: `a_list_of_indexes_is_searched_as_one` in crates/node/tests/it/search_several.rs (seen red before the change, green after)
- AC-2: `an_alias_of_several_is_the_list_and_a_repeat_is_searched_once` in crates/node/tests/it/search_several.rs
- AC-3: `a_name_that_is_no_index_refuses_the_search` in crates/node/tests/it/search_several.rs (it passed before the change too: the old path was also a 404, so it was not seen red)
- AC-4: `a_size_merges_across_the_indexes` in crates/node/tests/it/search_several.rs
- AC-5: `collapse_and_a_view_are_refused_over_several_indexes` in crates/node/tests/it/search_several.rs
- AC-6: `one_name_answers_as_before` in crates/node/tests/it/search_several.rs
