# Filesystem store: verified

Run with `cargo test -p oss-blob --test fs --test fs_streaming_peak` and the whole gate, `scripts/gate.sh`, on 2026-11-15 (Linux sandbox, ext4-backed temporary directory). Each listed defect was planted and seen to fail its test: a PUT written in place (`no_reader_sees_a_key_part_written`), a create published by rename instead of a hard link (`of_racing_creates_exactly_one_wins_and_its_body_is_stored`), files staged next to the key (`staged_files_are_never_listed`), no short-body check (`a_put_with_a_bad_body_leaves_the_old_object_and_no_staged_file`), a cursor pruning a directory it should enter (`a_page_starts_after_the_cursor`), a page heap without its extra key (`listing_pages_through_2500_keys_in_3_lists`).

- AC-1: `each_operation_records_exactly_one_attempt_of_its_kind`, `a_failed_call_is_one_attempt_and_one_error`
- AC-2: `a_key_is_a_file_under_the_root_holding_exactly_the_body`, `a_directory_the_java_shell_wrote_reads_and_lists`
- AC-3: `a_filesystem_failure_is_a_failed_attempt`, `a_ranged_get_returns_exactly_the_bytes_asked_for`, `a_get_streams_through_the_caller_buffer`, `a_directory_is_no_object`, `a_sink_failure_ends_the_get_and_counts_as_an_error`
- AC-4: `a_put_replaces_the_object_whole`, `a_put_with_a_bad_body_leaves_the_old_object_and_no_staged_file`
- AC-5: `a_conditional_create_never_overwrites`, `of_racing_creates_exactly_one_wins_and_its_body_is_stored`
- AC-6: `no_reader_sees_a_key_part_written`
- AC-7: `head_and_delete_follow_s3`, `a_directory_is_no_object`
- AC-8: `a_page_is_the_next_keys_in_whole_key_byte_order`, `a_page_starts_after_the_cursor`, `listing_pages_through_2500_keys_in_3_lists`, `the_reserved_directory_is_never_listed`, `staged_files_are_never_listed`, `the_walk_stops_at_the_deepest_key`, `a_page_never_walks_a_directory_that_cannot_enter_it`, `a_page_of_exactly_the_remaining_keys_is_not_truncated`, `entries_that_are_not_objects_are_skipped`
- AC-9: `invalid_arguments_fail_before_any_attempt`, `components_are_checked`
- AC-10: `fs_is_the_default_and_needs_a_path`, `s3_needs_a_bucket_and_defaults_the_rest`, `another_type_is_refused`
- AC-11: `a_16_mib_object_streams_through_one_pooled_buffer`
- AC-12: `scripts/check-alloc.py`, `scripts/check-s3-access.py`
