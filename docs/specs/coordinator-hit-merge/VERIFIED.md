# Verified: the coordinator merges the hits of every shard into one page

- AC-1: `no_sort_orders_by_score_stably_with_unscored_hits_last` in crates/coordinator/tests/hit_merge.rs
- AC-2: `field_sorts_run_in_their_direction_with_missing_values_placed_by_the_sort` in crates/coordinator/tests/hit_merge.rs
- AC-3: `sort_values_of_different_kinds_are_refused_when_compared` in crates/coordinator/tests/hit_merge.rs
- AC-4: `totals_relation_best_score_and_flags_combine` in crates/coordinator/tests/hit_merge.rs
- AC-5: `the_pile_stays_within_two_windows_and_the_page_is_unchanged` in crates/coordinator/tests/hit_merge.rs
- AC-6: `the_page_is_from_and_size_of_the_sorted_pile` in crates/coordinator/tests/hit_merge.rs
- AC-7: `an_at_least_total_is_capped_by_the_tracked_bound` in crates/coordinator/tests/hit_merge.rs
