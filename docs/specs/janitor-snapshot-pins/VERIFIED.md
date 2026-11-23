# Verified

Emulator, so the request numbers are provisional.

- AC-1: `a_snapshot_of_the_index_keeps_every_blob_of_its_shards` in crates/node/tests/janitor_snapshot_pins.rs
- AC-2: `a_snapshot_of_another_index_or_incarnation_holds_nothing_of_this_one`, `a_shard_with_no_snapshot_at_all_is_swept` in the same file
- AC-3: `a_snapshot_record_that_cannot_be_read_stops_the_pass_and_deletes_nothing` in the same file
- AC-4: `reading_the_snapshots_costs_what_the_table_says` in the same file
- AC-5: `a_shard_of_a_snapshotted_incarnation_keeps_everything`, `another_incarnation_or_index_is_the_inner_pins_answer` in crates/janitor/src/gc/snapshot_pins.rs

- AC-6: `a_shard_a_snapshot_holds_is_not_looked_at_again_until_the_recheck` in crates/node/tests/janitor_snapshot_pins.rs
- AC-7: `more_snapshots_than_the_pass_may_hold_stop_it_and_delete_nothing` in the same file
