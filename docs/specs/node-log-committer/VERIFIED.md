# Node log committer: verification

- AC-1: `a_window_of_any_number_of_shards_is_one_put`, `no_records_write_nothing` (writer, `tests/node_log_committer.rs`, on the emulator's meter).
- AC-2: `windows_close_when_full_and_spill_in_order`, `records_past_a_windows_bytes_go_on_one_window_at_a_time`, `records_that_exactly_fill_a_window_share_it`, `a_shards_records_from_two_callers_in_one_window_are_one_slice` (writer).
- AC-3: `a_fence_ends_the_epoch_and_the_next_window_opens_another`, `a_lease_failure_fails_the_window_and_logs_nothing`, `an_append_of_unknown_outcome_ends_the_epoch_and_keeps_its_records_unpublished` (writer); `a_blob_that_cannot_be_encoded_fails_its_window_and_keeps_the_epoch`, `a_writer_that_panics_fails_its_window_and_leaves_the_committer_usable`, `a_put_that_panics_after_landing_ends_the_epoch_and_keeps_its_records_unpublished` (writer, `tests/node_log_committer_track.rs`).
- AC-4: `admission_is_checked_once_an_epoch_and_a_refusal_fails_only_its_shard` (writer).
- AC-5: `trimming_follows_publishes_across_epochs` (writer, `tests/node_log_committer_track.rs`); `the_trim_point_is_the_lowest_unpublished_run_or_the_next_slot` (unit, `log/epochs.rs`).
- AC-6: `hedge_delay_is_off_at_zero_and_the_setting_until_puts_are_seen`, `hedge_delay_is_at_least_the_95th_percentile_of_recent_puts`, `hedge_delay_remembers_only_the_last_128_puts`, `hedge_delay_scales_a_blob_larger_than_the_median` (unit, `log/hedge_delay.rs`); `a_hedged_window_counts_its_hedges` (writer, `tests/node_log_committer_track.rs`).
- AC-7: `enqueued_records_ride_the_next_blob`, `an_enqueued_record_that_does_not_land_is_not_told` (writer, `tests/node_log_committer_track.rs`).
- AC-8: `idle_flush_writes_a_lone_record_at_once` (writer, `tests/node_log_committer_track.rs`); `the_node_log_settings_form_a_valid_registry_with_the_javas_defaults` (unit, `log/settings.rs`).
- Errors: `errors_name_what_failed` (writer, `tests/node_log_committer_track.rs`).
