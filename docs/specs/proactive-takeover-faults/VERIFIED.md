# Verified: proactive-takeover-faults

- AC-1: `with_the_switch_on_a_fleet_with_no_death_makes_no_request` in crates/node/tests/proactive_evidence.rs
- AC-2: `every_shard_the_log_names_is_taken_in_one_pass` in crates/node/tests/proactive_faults.rs
- AC-3: `a_shard_published_before_the_last_blob_is_not_named_and_not_touched` in crates/node/tests/proactive_faults.rs
- AC-4: `a_take_that_fails_leaves_the_shard_to_the_next_write_with_nothing_lost` in crates/node/tests/proactive_faults.rs
- AC-5: `a_take_whose_answer_was_lost_is_finished_by_the_next_pass` in crates/node/tests/proactive_faults.rs
- AC-6: `two_reapers_racing_leave_each_shard_held_once` in crates/node/tests/proactive_faults.rs
- AC-7: `a_shard_a_live_node_holds_is_left_alone_and_not_run_again` in crates/node/tests/proactive_faults.rs
- AC-8: `an_index_recreated_under_another_uuid_is_skipped` in crates/node/tests/proactive_faults.rs
- AC-9: `a_deadline_stops_new_takes_and_the_rest_stay_lazy` in crates/node/tests/proactive_faults.rs
- AC-10: `a_pass_takes_no_more_shards_at_once_than_max_concurrent` in crates/node/tests/proactive_faults.rs
- AC-11: `a_header_over_the_cap_is_refused_and_its_shards_stay_lazy` in crates/node/tests/proactive_faults.rs
- AC-12: `the_pass_costs_one_request_a_death_over_the_writes_that_would_have_taken_the_shards` in crates/node/tests/proactive_evidence.rs
- AC-14: `the_dead_log_pass_reuses_the_fence_the_proactive_pass_placed` in crates/node/tests/proactive_faults.rs
- AC-15: `a_node_that_died_returned_and_died_again_is_read_by_its_new_epochs` in crates/node/tests/proactive_faults.rs
- AC-16: `a_write_placed_on_another_node_reaches_the_reaper_that_took_the_shard` in crates/node/tests/proactive_faults.rs
- AC-17: `a_pass_costs_what_the_table_says` in crates/node/tests/proactive_evidence.rs
- AC-18: `a_reader_sees_the_same_documents_after_a_pass_as_after_a_write_takes_the_shard` in crates/node/tests/proactive_evidence.rs
- AC-13: `write_latency_of_a_live_node_while_a_pass_runs` in crates/node/tests/proactive_evidence.rs, run by hand: 5 runs of 300 writes each side, emulator GET 20-30 ms, PUT 40-60 ms (provisional): p50 75.8 vs 76.0 ms, p99 89.2 vs 88.8 ms, ratio 0.996.
