# Mapping growth: verification

- AC-1: `growing_writes_the_merged_mapping_and_moves_the_version_by_one` (writer, `tests/mapping_growth.rs`).
- AC-2: `an_addition_the_mapping_already_has_swaps_nothing` (writer, `tests/mapping_growth.rs`).
- AC-3: `a_lost_swap_reads_again_so_both_writers_keep_their_fields`, `the_waits_follow_the_javas_bounds` (writer, `tests/mapping_growth.rs`).
- AC-4: `five_lost_swaps_fail_with_concurrent_after_four_waits` (writer, `tests/mapping_growth.rs`).
- AC-5: `a_refused_merge_is_a_conflict_after_one_read_and_poisons_nothing` (writer, `tests/mapping_growth.rs`).
- AC-6: `an_index_that_is_gone_or_recreated_is_deleted` (writer, `tests/mapping_growth.rs`).
- AC-7: `concurrent_callers_coalesce_and_each_keeps_its_field`, `an_incompatible_addition_in_a_batch_refuses_only_its_caller`, `a_batch_the_store_failed_goes_back_to_the_callers_waiting_on_it`, `a_store_failure_with_nobody_waiting_leaves_the_mapping_alone` (writer, `tests/mapping_growth.rs`).
