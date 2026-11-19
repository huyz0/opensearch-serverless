# Janitor point-in-time pins: verified

Run on 2026-11-19 in a Linux sandbox, emulator numbers provisional: `cargo test -p oss-janitor --test point_in_time_pins` and `cargo test -p oss-format`.

- AC-1: `no_views_pin_no_blobs`.
- AC-2: `a_view_pins_each_blob_its_files_are_in_whatever_the_location_form`.
- AC-3: `views_of_another_index_incarnation_or_shard_pin_nothing_here`, `a_view_written_without_a_uuid_pins_by_name` and `the_files_of_several_views_are_all_pinned`.
- AC-4: `an_unfinished_capture_of_the_index_pins_everything_and_one_of_another_does_not`.
- AC-5: `a_view_that_cannot_be_read_pins_everything` and `a_location_no_reader_could_follow_pins_everything`.
- AC-6: `a_snapshot_costs_a_listing_and_a_read_a_view_and_answering_costs_nothing`.
- AC-7: `a_sweep_keeps_what_a_view_names_until_the_view_is_released`.
- AC-8: `a_snapshot_answers_each_shard_once_so_a_view_made_after_it_cannot_lose_its_files`.
- AC-9: `an_expired_view_pins_nothing_and_a_kept_alive_one_still_does`.
- AC-10: `every_entry_of_a_shard_a_record_names_is_pinned` and `more_live_views_than_the_cap_fail_the_pass_closed`; also `a_capture_of_another_incarnation_of_the_index_still_holds_it` (AC-4).
