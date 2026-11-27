# Verified: the reader asks a staging writer for its newest commit

- AC-1: `a_later_stage_of_the_same_writer_and_term_replaces_the_register` in crates/reader/tests/it/commit_push.rs
- AC-2: `a_register_commit_that_was_not_staged_is_never_asked_about` in crates/reader/tests/it/commit_push.rs
- AC-3: `a_cold_open_takes_the_newer_commit_its_writer_staged_with_the_same_gets` in crates/node/tests/server/newest_ask.rs
- AC-4: `no_answer_or_an_older_commit_leaves_the_open_on_the_register` in crates/node/tests/server/newest_ask.rs
- AC-5: `a_live_writer_is_asked_once_signed_and_its_answer_decoded` in crates/node/tests/tail/newest_ask_transport.rs
- AC-6: `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs
- AC-6: also `a_cold_open_takes_the_newer_commit_its_writer_staged_with_the_same_gets` in crates/node/tests/server/newest_ask.rs (install on and off) and `without_a_source_a_cold_open_is_as_before` (the default)
- AC-7: `a_source_that_takes_the_whole_timeout_costs_a_cold_open_that_long_and_no_more` in crates/node/tests/server/newest_ask.rs
