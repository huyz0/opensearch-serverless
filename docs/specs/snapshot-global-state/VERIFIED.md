# Verified: snapshot-global-state

- AC-1: `what_is_written_is_read_back_and_an_empty_state_writes_nothing`, `an_unknown_member_is_skipped`.
- AC-2: `the_record_the_java_wrote_is_read_in_full`.
- AC-3: `a_malformed_state_is_refused`.
- AC-4: `a_state_of_exactly_the_limit_is_taken_and_one_byte_more_names_the_set`, `settings_that_pass_the_limit_name_the_settings`, `a_state_past_the_limit_is_refused_on_read`.
- AC-5: `a_take_that_asks_records_the_settings_and_every_set`.
- AC-6: `without_the_flag_or_the_setting_nothing_is_recorded_and_the_answer_says_false`, `a_take_captures_no_global_state_until_set`.
- AC-7: `an_empty_state_writes_no_member`.
- AC-8: `a_state_past_the_limit_is_refused_naming_the_set_and_costs_no_shard_read`.
- AC-9: `a_take_with_state_costs_what_the_budget_holds`.
- AC-10: `a_restore_puts_the_documents_back_and_merges_the_settings_without_deleting`.
- AC-11: `without_the_flag_the_state_is_left_alone`.
- AC-12: `a_snapshot_without_state_answers_zero_and_no_indices_applies_only_the_state`.
- AC-13: `a_document_that_no_longer_validates_is_named_and_the_rest_are_applied`, `each_set_applies_the_validator_of_its_put_route`.
- AC-14: `a_set_past_a_thousand_documents_is_refused_and_exactly_a_thousand_is_not`, `the_refusal_is_a_400_naming_the_set`.
- AC-15: `a_background_restore_applies_the_state_after_the_indices`, `a_cancelled_global_state_step_names_the_count_and_applies_nothing`.
- AC-16: `a_restore_of_the_state_costs_what_the_budget_holds`.
- AC-17: `the_answer_names_what_was_applied_and_what_failed`.
- AC-18: `a_take_writes_the_global_state_record_a_java_node_reads` (crates/node/tests/server/snapshot_global_take.rs) wrote `fixtures/snapshots/rust-take-global/state.bin`; `fixtures/snapshots/ServerlessRustGlobalStateTests.java`, copied into `:serverless:testkit`, ran against the shell at `256a2a6d` (#189): 3 tests, 0 skipped, 0 failures (`-Dtests.rust.global=<dir>`; run by hand). Both sides write a setting as the register's text, so a number or boolean reads back as a string.
- AC-19: `a_set_that_changes_mid_walk_is_read_again_and_each_document_is_held_once` (crates/node/tests/server/snapshot_global_race.rs): holds the 8th whole-object read, deletes a script, releases; the take costs more requests than the same take over the final set and holds scripts a and b once. Seen red: with the walk's `found.clear()` on restart commented out the test fails (a document written twice); source restored.
- AC-20: `a_failing_settings_merge_is_named_and_leaves_the_settings` (crates/node/tests/server/snapshot_global_restore.rs).
- AC-21: `a_thread_that_panics_after_the_indices_are_whole_keeps_them_and_frees_its_slot` (crates/node/src/snapshot_restore_background.rs). It exercises the guard that is dropped by a panic, not a panic inside the apply step, which has no fault point and no path that panics today. A panic there is not counted as a failure (the guard counts only when armed).
- AC-22: `fixtures/snapshots/ServerlessRustGlobalRestoreTests.java`, copied into `:serverless:testkit` and run against the shell at `256a2a6d` over the bucket written with `OSS_WRITE_BUCKET`: 1 test, 0 skipped, 0 failures; the Java restore answered `global_state_restored: 7` (two settings, five documents) and read each back through its REST route.

None of these tests was seen red first, except AC-19's: they were written with the code. NOT-RUN: a state whose record plus shards passes 16 MiB (the existing 413 at the final write answers it), a panic inside the apply step itself (AC-21 covers the guard, not that call), the settings merge as one swap (a failure names `persistent_settings` as one entry, not each key).
