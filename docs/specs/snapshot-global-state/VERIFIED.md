# Verified: snapshot-global-state

- AC-1: `what_is_written_is_read_back_and_an_empty_state_writes_nothing`, `an_unknown_member_is_skipped`.
- AC-2: `the_record_the_java_wrote_is_read_in_full`.
- AC-3: `a_malformed_state_is_refused`.
- AC-4: `a_state_of_exactly_the_limit_is_taken_and_one_byte_more_names_the_set`, `settings_that_pass_the_limit_name_the_settings`, `a_state_past_the_limit_is_refused_on_read`.

None of these tests was seen red first: they were written with the code. NOT-RUN: a Java node reading a Rust-written record with global state (`Check.java` was not rebuilt here), and the take and restore criteria, which come in the next changes.
