# Node log replay, part 1: verification

- AC-1: `a_successor_reads_every_unpublished_record_of_its_shard_across_epochs` (writer, `tests/node_log_replay.rs`).
- AC-2: `shards_in_one_blob_are_read_with_one_read_of_it` (writer, on the emulator's meter).
- AC-3: `records_after_a_position_and_in_known_runs` (writer).
- AC-4: `a_consumer_stops_the_read_and_a_failed_body_read_keeps_nothing` (writer).
- AC-5: `a_body_is_kept_only_when_shared_and_within_its_bounds`, `bodies_are_let_go_once_kept_past_their_time` (writer).
- AC-6: `a_log_the_shell_wrote_on_fs_replays_as_the_shell_reads_it` (writer, `tests/node_log_cross_replay.rs`).
