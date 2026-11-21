# Verified: a node removes the tombstones of deleted indexes

- AC-1: `a_node_that_is_not_the_sweeper_makes_no_request` in crates/node/tests/tombstone_pass.rs
- AC-2: `the_sweeper_waits_out_its_interval_then_removes_what_is_old_and_keeps_what_is_young` in crates/node/tests/tombstone_pass.rs
- AC-3: `the_sweeper_waits_out_its_interval_then_removes_what_is_old_and_keeps_what_is_young` in crates/node/tests/tombstone_pass.rs
- AC-4: `the_sweeper_waits_out_its_interval_then_removes_what_is_old_and_keeps_what_is_young` in crates/node/tests/tombstone_pass.rs
- AC-5: `a_sweep_with_nothing_deleted_is_one_list` in crates/node/tests/tombstone_pass.rs
- AC-6: `a_store_that_does_not_honour_a_conditional_delete_is_never_swept` in crates/node/tests/tombstone_pass.rs
- AC-7: `a_pass_with_no_memory_for_its_buffer_sweeps_nothing_and_tries_again` in crates/node/tests/tombstone_pass.rs
- AC-8: `an_interval_of_zero_turns_the_pass_off` in crates/node/tests/tombstone_pass.rs
- AC-9: `the_role_moves_to_the_next_node_when_the_sweeper_leaves` in crates/node/tests/tombstone_pass.rs
- AC-10: `a_booted_node_removes_the_old_tombstone_of_a_deleted_index` in crates/node/tests/boot_tombstone.rs
  and `the_tombstone_sweep_is_hourly_unless_set_and_zero_turns_it_off` in crates/node/tests/boot_tombstone.rs
