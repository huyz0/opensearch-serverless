# Verified

Emulator behind the in-process S3 front end, so request counts are exact and any timing is provisional. The switch stays off; the cost soak (R11.5) decides the default.

- AC-1: `a_taken_shard_with_nothing_more_to_replay_is_adopted_by_one_swap` and `a_taken_shard_replays_only_what_the_logged_commit_lacks` in crates/node/tests/roles/writer_open_logged.rs; `a_newer_commit_is_chosen_and_carries_the_register_generation` in crates/writer/tests/takeover_logged.rs
- AC-2: `an_adopted_commit_is_swapped_in_by_the_successors_publish_with_no_upload` and `a_swap_lost_after_adopting_is_not_retried_over_the_register_that_moved` in crates/writer/tests/publish_adopt.rs; `a_failed_read_during_the_open_loses_nothing` in crates/node/tests/roles/writer_open_logged.rs
- AC-3: `a_commit_not_newer_than_the_register_is_ignored`, `the_registers_without_a_stage_sequence_or_absent_refuse_every_candidate`, `a_candidate_must_come_from_a_term_before_the_heads`, `a_pack_age_is_judged_against_the_deadline_less_the_margin`, `a_record_with_no_age_or_no_manifest_is_ignored` and `across_writers_the_greatest_term_and_sequence_wins` in crates/writer/tests/takeover_logged.rs; `an_adopted_commit_past_the_pack_deadline_is_not_swapped_in` in crates/writer/tests/publish_adopt.rs
- AC-4: `with_the_setting_off_the_open_is_what_it_was` in crates/node/tests/roles/writer_open_logged.rs
- AC-5: `a_taken_shard_replays_only_what_the_logged_commit_lacks` in crates/node/tests/roles/writer_open_logged.rs (its takeover asserts no LIST)
- AC-6: `a_failed_read_during_the_open_loses_nothing` in crates/node/tests/roles/writer_open_logged.rs; `a_node_lost_with_staged_commits_loses_no_acknowledged_write` in crates/node/tests/roles/writer_staging.rs
- AC-7: `a_swap_lost_after_adopting_is_not_retried_over_the_register_that_moved` in crates/writer/tests/publish_adopt.rs (the returning predecessor's checkpoint first); `a_checkpoint_over_a_register_a_newer_term_holds_is_stale_and_leaves_nothing_staged` in crates/writer/tests/publish_staging.rs (the successor's swap first)
- AC-8: `opening_from_logged_commits_needs_logging` in crates/node/src/boot/config/commit_tests.rs; `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs
