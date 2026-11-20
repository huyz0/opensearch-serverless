# Writer takeover: verification

- AC-1: `a_replayed_record_is_a_live_document_with_its_own_identity_and_the_next_write_follows_it` and `a_replayed_delete_leaves_a_tombstone_a_create_may_pass` (writer, `tests/shard_replay.rs`). Seen failing against a replay that does nothing.
- AC-2: `a_stale_record_changes_nothing_and_an_unsequenced_one_takes_the_next_number` and `a_replayed_update_replaces_the_document_a_segment_holds`, `a_replayed_delete_removes_the_document_a_segment_holds` and `replayed_updates_and_deletes_leave_one_live_document_an_id_and_none_after_a_delete` (writer, `tests/shard_replay.rs`). Seen failing likewise.
- AC-3: `records_the_commit_holds_are_not_applied` and `only_records_above_the_commit_reach_the_engine_oldest_first` (writer, `tests/takeover.rs`): a0 in the commit, a1 and a2 applied.
- AC-4: `replayed_records_are_published_once_under_the_new_term_and_the_head_stops_naming_the_writer` (writer, `tests/takeover.rs`): 3 PUT (blob, register swap, head swap) after the batch's fence, 0 LIST, the register at term 2. Emulator, provisional.
- AC-5: `a_shard_with_nothing_to_replay_publishes_nothing` and `records_all_in_the_commit_publish_nothing` (writer, `tests/takeover.rs`): one PUT (the head swap) for a head naming a writer, none for a head naming none.
- AC-6: `a_batch_reads_each_predecessors_log_once_however_many_shards_it_takes` (writer, `tests/takeover.rs`): two shards, one factory call.
- AC-7: `replayed_records_are_published_once_under_the_new_term_and_the_head_stops_naming_the_writer` and `a_head_that_moved_on_is_not_written` and `a_head_is_not_written_unless_it_is_still_this_nodes_at_this_term` (writer, `tests/takeover.rs`).
- AC-8: `a_newer_term_in_the_register_refuses_the_publish_and_the_head_keeps_its_writers` (writer, `tests/takeover.rs`).

The takeover tests were seen failing against an adopt that replays nothing: all six.
- AC-9: `a_writer_with_no_log_is_still_named_by_the_head` (writer, `tests/takeover.rs`).
