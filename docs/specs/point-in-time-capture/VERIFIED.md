# Point-in-time capture: verified

Run on 2026-11-19 in a Linux sandbox, emulator numbers provisional: `cargo test -p oss-meta`. The Java formats were read from the code, not recorded from a Java run.

- AC-1: `every_shards_commit_is_frozen_under_a_fresh_id_and_the_view_reads_back`.
- AC-2: `every_shards_commit_is_frozen_under_a_fresh_id_and_the_view_reads_back` (no LIST) and `a_capture_costs_a_constant_and_one_read_a_shard`.
- AC-3: `a_shard_that_has_published_nothing_refuses_the_capture_and_leaves_nothing_pinned`.
- AC-4: `a_refusal_before_the_marker_writes_nothing` and `a_name_that_is_no_concrete_index_cannot_be_frozen`.
- AC-5: `a_bucketed_index_freezes_its_members_commit_for_the_cost_of_a_shards`.
- AC-5 also: `a_members_files_in_its_own_shard_path_are_frozen_as_recorded`.
- AC-10: `a_bucketed_capture_that_cannot_freeze_is_refused_and_leaves_nothing_pinned` and `a_register_that_names_another_bucket_is_not_the_members_bucket`. The bucket manifests are JSON the tests write; the compact form is `bucket-manifest-reader`'s.
- AC-6: `an_index_replaced_while_the_capture_began_is_refused_and_released`.
- AC-7: `the_same_name_made_again_after_the_capture_is_not_the_captured_view`.
- AC-8: `an_id_is_base64url_of_its_bytes_without_padding` and `new_ids_look_like_ids_and_differ`, unit tests in `crates/meta/src/pit/capture.rs`.
- AC-9: `an_index_in_units_freezes_its_live_units_and_no_retired_slot`, `a_live_unit_that_has_published_nothing_refuses_an_index_in_units` and `a_layout_that_cannot_be_read_is_refused_before_anything_is_written`.
- AC-10 also: `an_index_moved_to_another_bucket_while_the_capture_began_is_refused_and_released` and `a_bucket_register_larger_than_the_buffer_fails_the_capture_and_leaves_nothing_pinned`.
