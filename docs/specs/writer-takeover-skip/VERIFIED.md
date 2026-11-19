# Takeover reads only the log past the commit: verification

- AC-1: `only_slices_past_the_commit_are_handed_over` (writer, `tests/node_log_replay.rs`). Seen failing against a filter that skips nothing.
- AC-2: `slices_the_commit_covers_cost_no_body_or_slice_read` (writer, `tests/node_log_replay.rs`): 0 GET for a fully covered shard after the headers, 3 a slice read each without the skip, 1 for the one slice past the commit. Seen failing likewise. Emulator, provisional.
- AC-3: `a_slice_with_no_sequence_number_is_never_skipped`, `a_slice_mixing_unsequenced_and_sequenced_records_is_judged_by_the_sequenced_ones` and `a_shared_blob_hands_each_shard_only_its_slices_past_its_commit` (writer, `tests/node_log_replay.rs`).
- AC-4: `records_all_in_the_commit_publish_nothing` (writer, `tests/takeover.rs`): 3 GET (two headers and the head) against 6 when the replay reads every slice; no publish.
- AC-5: `the_engine_is_flushed_when_its_buffer_reaches_the_bound` and `only_records_above_the_commit_reach_the_engine_oldest_first` (writer, `tests/takeover_bounds.rs`), and `a_bound_of_zero_still_makes_progress_and_applies_every_record` and `a_flush_that_leaves_more_than_the_bound_does_not_flush_before_every_record` for the progress guarantee. Seen failing with the flush disabled.
- AC-6: `a_long_replay_holds_a_bounded_buffer_and_still_applies_and_publishes_every_record` (writer, `tests/takeover_bounds.rs`): 200 records, one publish, the bounded peak under half the unbounded one and within the bound plus one record's cost. Seen failing with the flush disabled.
