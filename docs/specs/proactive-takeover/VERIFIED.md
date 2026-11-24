# Verified: proactive takeover of a dead node's shards

- AC-1: `a_booted_node_takes_a_dead_nodes_shard_before_any_write_only_when_the_setting_is_on` in crates/node/tests/server/boot.rs
  and `with_the_switch_on_a_fleet_with_no_death_makes_no_request` in crates/node/tests/proactive_evidence.rs
- AC-2: `a_pass_takes_the_shard_the_dead_log_names_before_any_write` in crates/node/tests/proactive_takeover.rs
  and `every_shard_the_log_names_is_taken_in_one_pass` in crates/node/tests/proactive_faults.rs
- AC-3: `a_shard_published_before_the_last_blob_is_not_named_and_not_touched` in crates/node/tests/proactive_faults.rs
- AC-4: `only_the_reapers_act_each_after_its_turn_and_a_finished_pass_is_not_repeated` in crates/node/tests/proactive_takeover.rs
- AC-5: `only_the_reapers_act_each_after_its_turn_and_a_finished_pass_is_not_repeated` in crates/node/tests/proactive_takeover.rs
  (the turn); NOT-RUN: that the second reaper's checks cost at most twice the first's, and that it takes everything when the first is down
- AC-6: `a_pass_takes_no_more_shards_at_once_than_max_concurrent` in crates/node/tests/proactive_faults.rs
  and `a_header_over_the_cap_is_refused_and_its_shards_stay_lazy` in crates/node/tests/proactive_faults.rs
  (the header read is bounded by `max_header_bytes`; the pool-reservation clause was replaced by that bound, see the spec)
- AC-7: `a_deadline_stops_new_takes_and_the_rest_stay_lazy` in crates/node/tests/proactive_faults.rs
- AC-8: `only_the_reapers_act_each_after_its_turn_and_a_finished_pass_is_not_repeated` in crates/node/tests/proactive_takeover.rs
  and `a_node_that_died_returned_and_died_again_is_read_by_its_new_epochs` in crates/node/tests/proactive_faults.rs
- AC-9: `two_reapers_racing_leave_each_shard_held_once` in crates/node/tests/proactive_faults.rs
- AC-10: `a_dead_node_whose_lease_names_no_epochs_is_not_due_and_a_pass_makes_no_request` in crates/node/tests/proactive_takeover.rs
- AC-11: `the_dead_log_pass_reuses_the_fence_the_proactive_pass_placed` in crates/node/tests/proactive_faults.rs
  and `a_reaper_waits_out_a_log_that_is_still_needed_then_collects_it` in crates/node/tests/dead_log_pass.rs
  and `a_dead_lease_forgotten_after_its_log_is_collected_stops_costing_a_read_each_refresh` in crates/cluster/tests/leases/refresh.rs
- AC-12: `a_take_that_fails_leaves_the_shard_to_the_next_write_with_nothing_lost` in crates/node/tests/proactive_faults.rs
  and `a_dead_log_that_cannot_be_read_takes_nothing_and_stays_due` in crates/node/tests/proactive_takeover.rs
- AC-13: `an_index_recreated_under_another_uuid_is_skipped` in crates/node/tests/proactive_faults.rs
  and `a_shard_a_live_node_holds_is_left_alone_and_not_run_again` in crates/node/tests/proactive_faults.rs
- AC-14: `a_dead_node_whose_lease_names_no_epochs_is_not_due_and_a_pass_makes_no_request` in crates/node/tests/proactive_takeover.rs
- AC-15: `a_reaper_waits_out_a_log_that_is_still_needed_then_collects_it` in crates/node/tests/dead_log_pass.rs
  (a log a shard still needs is not collected, then is once a successor publishes)
  and `a_lease_dead_for_ten_lifetimes_is_unlisted_and_deleted_where_verified` in crates/cluster/tests/leases/refresh.rs
  (the forget swap is conditional on the epochs checked); NOT-RUN: the swap landing in the middle of a taker's re-read is not simulated
- AC-16: `a_pass_costs_what_the_table_says` in crates/node/tests/proactive_evidence.rs
  and `a_reapers_pass_costs_the_same_in_a_fleet_of_three_thirty_or_two_hundred` in crates/node/tests/proactive_evidence.rs
- AC-17: `proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read` in crates/node/src/boot/config.rs
  and `a_proactive_setting_out_of_its_bounds_is_refused` in crates/node/src/boot/config.rs; NOT-RUN: a Java-written log read by the pass (R8.7)
- AC-18: `a_write_placed_on_another_node_reaches_the_reaper_that_took_the_shard` in crates/node/tests/proactive_faults.rs
- AC-19: `the_dead_log_pass_reuses_the_fence_the_proactive_pass_placed` in crates/node/tests/proactive_faults.rs
  and `a_revoked_member_naming_epochs_is_reported_as_revoked` in crates/cluster/tests/leases/refresh.rs
  and `a_dead_member_naming_epochs_is_reported_on_every_refresh_with_no_extra_read` in crates/cluster/tests/leases/refresh.rs
- AC-20: `a_header_over_the_cap_is_refused_and_its_shards_stay_lazy` in crates/node/tests/proactive_faults.rs
- AC-21: `a_pass_that_failed_waits_a_stagger_and_then_twice_as_long` in crates/node/tests/proactive_takeover.rs
