# Janitor: collect a dead node's write log: verified

Run with `cargo test -p oss-janitor`, `cargo mutants -p oss-janitor` (59 mutants: 52 caught, 7 unviable, none missed) and the checks listed in the PR, on 2026-11-19 (Linux sandbox, emulator only, numbers provisional).

- AC-1: `nothing_is_judged_for_a_live_node_an_absent_lease_or_a_lease_without_epochs`
- AC-2: `a_dead_node_whose_shards_are_published_past_is_collected_lease_first`
- AC-3: `a_shard_the_commit_has_not_caught_up_with_keeps_the_log`, `a_log_holding_only_sequence_number_zero_is_not_published_past_a_checkpoint_of_minus_one`
- AC-4: `no_commit_no_checkpoint_an_owner_or_no_name_each_keep_the_log`
- AC-5: `a_deleted_or_recreated_index_and_a_shard_the_log_holds_nothing_of_need_nothing`
- AC-6: `a_deleted_or_recreated_index_and_a_shard_the_log_holds_nothing_of_need_nothing`
- AC-7: `a_dead_node_whose_shards_are_published_past_is_collected_lease_first`, `a_shard_the_commit_has_not_caught_up_with_keeps_the_log`
- AC-8: `a_node_that_comes_back_before_the_forget_keeps_its_log`
- AC-9: `a_dry_run_judges_the_same_and_changes_nothing`
- AC-10: `the_requests_are_the_lease_the_heads_and_descriptors_and_the_forget`
