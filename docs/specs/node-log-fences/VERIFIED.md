# Node log write fences: verification

- AC-1: `a_fenced_writer_lands_nothing_more` (writer, `tests/node_log_takeover.rs`).
- AC-2: `shards_taken_from_one_predecessor_share_its_fences` (writer, on the emulator's meter).
- AC-3: `a_fence_from_a_stale_lease_is_searched_for_again_above_the_deleted_slots`, `without_the_reread_a_stale_fence_lands_in_the_hole` (writer).
- AC-4: `an_epoch_with_nothing_above_its_trim_point_names_no_shard` (writer, on the emulator's meter).
- AC-5: `an_unfenced_reader_writes_nothing_and_the_writer_goes_on` (writer).
- AC-6: `a_failed_fence_or_reread_caches_nothing_and_the_next_call_succeeds`, `a_takeover_failure_says_what_failed` (writer).
