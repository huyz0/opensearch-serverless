# Verified: the orphan-shard sweep

Run with `cargo test -p oss-janitor -p oss-format` on the emulator, in a Linux sandbox; numbers are provisional. `scripts/mutants.sh` was not run on this change: NOT-RUN.

- AC-1: `a_budget_of_zero_is_refused` in crates/janitor/tests/orphans.rs (the timer half: NOT-RUN, with the wiring)
- AC-2: `a_container_of_a_gone_index_or_another_uuid_is_deleted_and_a_live_one_is_not` in crates/janitor/tests/orphans.rs
- AC-3: `a_container_whose_name_this_system_did_not_write_is_left` in crates/janitor/tests/orphans.rs
- AC-4: `a_descriptor_that_cannot_be_read_fails_the_slice_and_deletes_nothing` in crates/janitor/tests/orphans.rs
- AC-5: `a_point_in_time_or_a_snapshot_or_a_capture_pins_its_index` in crates/janitor/tests/orphans.rs
- AC-5: `pins_that_cannot_be_read_or_are_too_many_stop_the_slice_with_nothing_deleted` in crates/janitor/tests/orphans.rs
- AC-5b: `a_point_in_time_or_a_snapshot_or_a_capture_pins_its_index` in crates/janitor/tests/orphans.rs
- AC-5c: `a_pin_registered_after_the_descriptors_and_before_the_pins_read_holds_the_shard` in crates/janitor/tests/orphans.rs
- AC-5c: `a_descriptor_created_after_the_page_was_listed_is_not_deleted` in crates/janitor/tests/orphans.rs
- AC-5d: `every_captured_index_is_named_with_its_uuid` in crates/format/src/snapshot_pins.rs (a record written from the Java source; a recorded Java run: NOT-RUN)
- AC-6: `a_slice_that_loses_its_swap_stops_and_deletes_nothing` in crates/janitor/tests/orphans.rs (the rendezvous half: NOT-RUN, with the timer)
- AC-7: `slices_walk_every_container_in_order_and_the_last_ends_the_cycle` in crates/janitor/tests/orphans.rs
- AC-8: `a_dry_run_counts_the_orphans_and_deletes_nothing` in crates/janitor/tests/orphans.rs
- AC-9: `a_slice_costs_what_the_table_says_and_a_page_with_no_candidate_reads_no_pin` in crates/janitor/tests/orphans.rs
- AC-9: `an_orphan_costs_its_listing_its_deletes_and_the_pins_reads` in crates/janitor/tests/orphans.rs
- AC-10: `a_container_whose_name_this_system_did_not_write_is_left` in crates/janitor/tests/orphans.rs (a cursor value advanced from a recorded Java run: NOT-RUN)
- AC-11: `a_page_that_fails_three_times_is_skipped_counted_and_the_next_page_follows` in crates/janitor/tests/orphans.rs (the not-the-sweeper half: NOT-RUN, with the timer)
