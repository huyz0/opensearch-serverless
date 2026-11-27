# Verified: the coordinator merges a top_hits across shards

- AC-1: `the_hits_ahead_of_from_are_kept_for_the_coordinator_s_merge` in crates/reader/src/aggs/metric/top_hits_tests.rs
- AC-2: `the_page_is_the_best_hits_of_every_shard_after_from`, `a_hit_a_shard_ranked_ahead_of_from_can_be_in_the_page` in crates/coordinator/tests/it/top_hits_reduce.rs
- AC-3: `a_field_sort_follows_its_direction_and_a_tie_goes_to_the_lower_shard`, `a_missing_value_goes_where_the_sort_puts_it` in crates/coordinator/tests/it/top_hits_reduce.rs
- AC-4: `the_total_is_summed_and_the_best_score_is_the_highest` in crates/coordinator/tests/it/top_hits_reduce.rs
- AC-5: `sort_values_of_different_kinds_are_refused` in crates/coordinator/tests/it/top_hits_reduce.rs
- AC-6: `no_shards_or_no_hits_give_an_empty_page` in crates/coordinator/tests/it/top_hits_reduce.rs
- AC-7: `the_pile_never_holds_more_than_twice_the_window`, `the_pile_bound_counts_from_as_well_as_size` in crates/coordinator/tests/it/top_hits_reduce.rs
