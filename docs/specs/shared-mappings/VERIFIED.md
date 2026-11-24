# Verified

- AC-1: `equal_text_is_one_mapping_and_unequal_text_is_two`, `a_text_that_does_not_parse_is_an_error_and_is_not_stored`
- AC-2: `a_dropped_mapping_is_freed_and_churn_does_not_grow_the_table`
- AC-3: `shards_of_indexes_with_one_mapping_hold_one_parsed_copy`
- AC-4: `a_shard_that_grows_leaves_the_shared_mapping_to_the_others`
- AC-5: `many_threads_share_one_mapping_and_distinct_texts_do_not_wait`
- AC-6: NOT-RUN: holds by construction, the table in `crates/doc/src/mapping/intern.rs` holds no store
- AC-7: `a_hit_costs_far_less_than_a_parse`
