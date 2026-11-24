# Verified

Emulator and a file store, so request numbers are provisional.

- AC-1: `a_release_clears_the_owner_and_keeps_the_term_and_writers` in crates/cluster/tests/leases/heads.rs; `a_shard_asleep_long_enough_gives_its_head_back_for_what_the_table_says` in crates/node/tests/writer_give_back.rs (request-budget row `writer.hibernation.give_back.requests`)
- AC-2: `a_release_leaves_a_head_that_is_not_this_processes_at_that_term` in crates/cluster/tests/leases/heads.rs
- AC-3: `a_shard_asleep_long_enough_gives_its_head_back_for_what_the_table_says`, `a_shard_asleep_less_is_left_and_a_pass_over_none_due_asks_nothing` in crates/node/tests/writer_give_back.rs
- AC-4: `with_the_threshold_zero_nothing_is_given_back` in the same file
- AC-5: `a_write_after_a_give_back_takes_the_shard_at_the_next_term_with_every_document` in the same file
- AC-6: `another_nodes_write_takes_a_given_back_head_and_this_node_is_then_refused` in the same file
- AC-7: `a_write_that_woke_the_shard_first_leaves_nothing_to_give_back`, `a_head_that_moved_while_it_slept_is_left_as_the_other_node_wrote_it` in the same file
- AC-8: `a_booted_node_gives_back_the_head_of_a_long_asleep_shard_only_with_both_settings_on` in crates/node/tests/server/boot_give_back.rs
- AC-9: `a_take_that_loses_the_swap_to_a_release_wins_the_unowned_head_at_once`, `a_head_naming_a_node_alone_is_not_given_back` in crates/cluster/tests/leases/heads.rs
- AC-10: `a_pass_gives_back_no_more_than_its_cap_and_the_rest_are_due_next_pass` in crates/node/tests/writer_give_back.rs; `a_give_back_threshold_beyond_the_day_a_record_is_kept_is_refused` in crates/node/src/boot/config/tests.rs
- AC-11: `the_give_back_threshold_defaults_to_never_and_is_set_by_the_variable` in crates/node/src/boot/config/tests.rs

Not measured: the write latency or request totals of a fleet with the setting on, which is why it stays off. The race of a take against a release is covered deterministically by AC-9's hook; a concurrent run of a wake against a give-back is not.
