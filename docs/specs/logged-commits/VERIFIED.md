# Verified

Emulator behind the in-process S3 front end, so request counts are exact and any timing is provisional. The writing half only: nothing reads a logged commit yet, and the switch stays off.

- AC-1: `a_staged_commit_reads_back_with_its_pack_time_and_position` and `a_commit_with_no_covered_position_round_trips` in crates/writer/tests/it/log_commit.rs
- AC-2: `a_logging_node_makes_no_write_for_a_staged_commit` and `a_quiet_node_writes_once_after_two_windows_for_every_shard_waiting` in crates/writer/tests/it/log_commit.rs
- AC-3: `a_commit_still_queued_when_its_shard_is_let_go_is_released_as_it_lands`, `only_the_newest_commit_stays_held_until_the_register_covers_it` and `a_checkpoint_before_the_newest_keeps_the_newest` in crates/writer/tests/it/log_commit.rs; `a_staged_commit_is_logged_and_released_when_the_register_is_written` in crates/node/tests/roles/writer_logged.rs
- AC-4: `a_record_older_than_the_register_is_released_as_it_lands` in crates/writer/tests/it/log_commit.rs
- AC-5: `a_commit_key_is_not_a_shard` and `a_successor_is_not_told_of_a_commit_key_as_a_shard` in crates/writer/tests/it/log_commit.rs
- AC-6: `with_logging_off_nothing_is_logged_and_no_request_is_added` in crates/node/tests/roles/writer_logged.rs
- AC-7: `a_successor_is_not_told_of_a_commit_key_as_a_shard` in crates/writer/tests/it/log_commit.rs (the janitor reads the same list)
- AC-8: `commit_logging_is_off_by_default_and_needs_staging` in crates/node/src/boot/config/commit_tests.rs; `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs
