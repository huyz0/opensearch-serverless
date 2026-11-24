# Verified: hibernating a writer that has gone idle

- AC-1: `a_pass_with_the_setting_off_hibernates_nothing_and_asks_nothing` in crates/node/tests/writer_hibernation.rs
- AC-2: `an_idle_published_shard_sleeps_keeping_its_head_and_asking_nothing` in crates/node/tests/writer_hibernation.rs
- AC-3: `a_shard_used_within_the_threshold_stays_open` in crates/node/tests/writer_hibernation.rs
- AC-4: `a_shard_with_unpublished_writes_is_published_then_sleeps` in crates/node/tests/writer_hibernation.rs
- AC-5: `a_write_to_a_sleeping_shard_wakes_it_at_the_same_term_with_every_document` in crates/node/tests/writer_hibernation.rs
  and `waking_costs_what_the_table_says_and_less_than_taking_the_shard_over` in crates/node/tests/writer_hibernation.rs
- AC-6: `a_shard_whose_head_moved_while_it_slept_is_opened_the_ordinary_way` in crates/node/tests/writer_hibernation.rs
- AC-7: `a_write_that_took_the_slot_just_before_it_slept_wakes_the_shard_and_lands` in crates/node/tests/writer_hibernation.rs
  and `writes_interleaved_with_passes_that_sleep_the_shard_all_land` in crates/node/tests/writer_hibernation.rs
- AC-8: `a_sleeping_shard_refreshes_without_a_request_and_a_get_wakes_it` in crates/node/tests/writer_hibernation.rs
- AC-9: `the_publish_timer_hibernates_an_idle_published_shard_only_when_the_threshold_is_set` in crates/node/tests/server/assemble.rs
- AC-10: `hibernation_is_off_unless_the_variable_sets_an_idle_time` in crates/node/tests/server/boot.rs
- AC-11: `a_record_of_a_shard_asleep_for_a_day_is_forgotten_without_a_request` in crates/node/tests/writer_hibernation.rs
