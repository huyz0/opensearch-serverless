# Point-in-time store: verified

Run on 2026-11-19 in a Linux sandbox, emulator numbers provisional:
- `cargo test -p oss-format point_in_time`
- `cargo test -p oss-meta --test descriptors point_in_time`

The Java bytes in the format tests were written by hand from reading `PointInTime.toBytes` at `429b4cbc`; no Java run recorded them, so they fix the field order and spelling that code produces but are not captured output.

- AC-1: `bytes_the_java_writes_are_read_and_written_back_the_same`, `a_record_without_a_uuid_or_lengths_reads_and_writes_back` and `a_capture_marker_holds_no_shards_and_says_so` (format, `tests/point_in_time.rs`). Seen failing before a flipped marker rule was fixed.
- AC-2: `malformed_records_are_refused`, `views_and_markers_are_told_apart_by_their_shards`, `a_shard_given_twice_is_refused`, `a_record_past_the_bound_is_refused_before_it_is_read`, `a_shard_past_the_file_bound_is_refused`, `the_released_value_is_not_a_view` (format).
- AC-2 also: `a_record_of_exactly_the_bound_is_read` (format). AC-8 also: `a_placeholder_names_no_index` (format) and `a_listing_longer_than_a_page_is_read_to_its_end` (meta).
- AC-3: `a_view_pins_its_index_by_uuid_when_both_know_one_and_by_name_otherwise` and `a_view_expires_at_its_deadline_and_extends_keeping_everything_else` (format).
- AC-4: `a_created_view_reads_back_and_the_id_is_taken` (meta, `tests/descriptors/point_in_time.rs`; the races in `point_in_time_races.rs`).
- AC-5: `a_capture_is_not_readable_until_it_is_finished_under_the_same_id` and `finishing_refuses_a_capture_that_is_gone_another_index_or_itself_a_capture` (meta).
- AC-6: `a_keep_alive_only_moves_the_deadline_later`, `a_capture_is_not_extended`, `an_unconditional_keep_alive_that_loses_a_race_retries_and_a_conditional_one_gives_up` and `a_keep_alive_that_loses_every_attempt_is_contended` (meta).
- AC-7: `a_released_id_cannot_be_created_read_or_released_again` (meta).
- AC-8: `live_views_are_listed_and_expired_ones_are_claimed_and_removed` and `a_view_that_cannot_be_read_stands_in_as_a_placeholder` (meta).
- AC-9: `a_view_written_before_the_state_register_is_adopted_by_a_read` and `a_legacy_entry_under_another_id_is_refused_and_not_adopted` (meta), `a_state_under_another_id_is_refused`.
- AC-11: `a_record_that_no_reader_would_take_is_not_stored` (meta).
- AC-12: `a_collector_claiming_expiry_loses_to_a_keep_alive_that_wins_first` (meta).
- AC-13: `keep_alive_finish_and_expiry_cost_what_the_spec_says` and `a_created_view_reads_back_and_the_id_is_taken` (meta).
- AC-10 also: `a_marker_that_failed_is_kept_when_the_view_turned_out_to_be_live_under_another_writer` and `a_keep_alive_rewrites_a_marker_whose_deadline_is_not_the_one_collectors_read_as_never` (meta).
- AC-10: `a_marker_already_there_is_not_removed_by_a_create_that_lost_the_id` (meta) and `a_marker_that_cannot_be_written_releases_the_view_it_began` (meta).
