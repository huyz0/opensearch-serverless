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

None of these tests was seen red first: they were written with the code. NOT-RUN: a Java node reading a Rust-written record with global state (`Check.java` was not rebuilt here), a document set restarting mid-walk (the restart logic in `read_set` has no test), a state whose record plus shards passes 16 MiB (the existing 413 at the final write answers it), and the restore criteria, which come in the next change.
