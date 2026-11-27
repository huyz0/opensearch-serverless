# Verified: the coordinator walks a delete- or update-by-query one shard at a time

- AC-1: `shards_are_walked_in_order_and_an_empty_one_is_not_paged` in crates/coordinator/tests/it/by_query.rs
- AC-2: `pages_follow_the_last_number_and_are_bounded_by_the_start` in crates/coordinator/tests/it/by_query.rs
- AC-3: `the_walk_stops_at_max_docs_across_shards` in crates/coordinator/tests/it/by_query.rs
- AC-4: `changes_noops_conflicts_and_batches_are_counted` in crates/coordinator/tests/it/by_query.rs
- AC-5: `aborting_on_conflict_ends_at_the_hit_with_its_message` in crates/coordinator/tests/it/by_query.rs
- AC-6: `a_refresh_rides_only_the_last_hit_of_a_shards_last_page` in crates/coordinator/tests/it/by_query.rs
- AC-6 (the cap): `reaching_max_docs_makes_that_page_the_last_so_the_refresh_rides_it` in crates/coordinator/tests/it/by_query.rs
- AC-7: `a_page_is_admitted_by_its_bytes_and_held_while_it_is_changed` and `a_refused_admission_a_failed_page_and_a_failed_change_end_the_walk` in crates/coordinator/tests/it/by_query.rs
