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

None of these tests was seen red first: they were written with the code. NOT-RUN: a Java node reading a Rust-written record with global state (`Check.java` was not rebuilt here), a document set restarting mid-walk (the restart logic in `read_set` has no test), a state whose record plus shards passes 16 MiB (the existing 413 at the final write answers it), a panic inside the background step (it runs after the undo guard is disarmed, as the alias step does, and no test panics it), the settings merge as one swap (a failure names `persistent_settings` as one entry, not each key), and a Java node restoring a Rust snapshot's state.
