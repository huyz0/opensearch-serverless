# Verified

Emulator behind the in-process S3 front end, so request counts are exact and any timing is provisional. The cost soak (write p99, takeover, requests per publish at 1,000 shards) is R11.5 and is not run here; the switch stays off.

- AC-1: `with_staging_off_a_routine_publish_is_a_publish` in crates/writer/tests/it/publish_staging.rs
- AC-2: `the_first_routine_publish_of_a_term_writes_the_register` in crates/writer/tests/it/publish_staging.rs
- AC-3: `a_routine_publish_inside_the_checkpoint_age_makes_one_blob_put_and_no_swap` in crates/writer/tests/it/publish_staging.rs
- AC-4: `staged_files_are_not_uploaded_again` in crates/writer/tests/it/publish_staging.rs
- AC-5: `a_routine_publish_at_the_checkpoint_age_swaps_with_every_file` in crates/writer/tests/it/publish_staging.rs
- AC-6: `a_checkpoint_after_stages_writes_the_newest_with_one_put_and_no_upload` in crates/writer/tests/it/publish_staging.rs
- AC-7: `a_full_publish_after_stages_writes_everything_and_uploads_nothing_twice` in crates/writer/tests/it/publish_staging.rs
- AC-8: `a_staged_commit_past_the_pack_deadline_is_dropped_and_its_files_uploaded_again` in crates/writer/tests/it/publish_staging.rs
- AC-9: `a_checkpoint_over_a_register_a_newer_term_holds_is_stale_and_leaves_nothing_staged` in crates/writer/tests/it/publish_staging.rs
- AC-10: `the_stage_sequence_rises_with_every_commit_while_staging_is_on` in crates/writer/tests/it/publish_staging.rs
- AC-11: `the_checkpoint_age_has_the_shells_bounds` in crates/writer/src/publish/staging.rs; `a_commit_is_due_at_exactly_the_checkpoint_age` in the same file
- AC-12: `the_settings_default_to_off_and_a_minute` in crates/writer/src/publish/staging.rs; `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs; `commit_staging_is_off_by_default_and_set_by_its_two_variables` in crates/node/src/boot/config/tests.rs
- AC-13: `a_staged_publish_trims_nothing_and_leaves_the_shard_unpublished_until_the_register_is_written` and `a_checkpoint_writes_the_register_and_trims_the_log` in crates/node/tests/roles/writer_staging.rs
- AC-14: `a_node_lost_with_staged_commits_loses_no_acknowledged_write` in crates/node/tests/roles/writer_staging.rs
