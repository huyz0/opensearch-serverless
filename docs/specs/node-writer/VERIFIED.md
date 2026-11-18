# Verified: node writer role

- AC-1: `a_write_returns_after_one_blob_holding_its_record` in crates/node/tests/writer_role.rs
- AC-2: `the_first_write_records_one_epoch_and_later_ones_none` in crates/node/tests/writer_role.rs
- AC-3: `a_group_of_fifty_is_one_blob_and_two_shards_are_one_blob` in crates/node/tests/writer_role.rs
- AC-4: `a_refused_write_logs_nothing_and_takes_no_sequence_number` in crates/node/tests/writer_role.rs
- AC-5: `a_failed_stage_refuses_the_write_and_drops_the_shard` in crates/node/tests/writer_role.rs
- AC-6: `a_foreign_shard_is_forwarded_once_and_logs_nothing_here` in crates/node/tests/writer_role.rs
- AC-7: `a_shard_nobody_holds_fails_and_logs_nothing` in crates/node/tests/writer_role.rs
- AC-8: `deletes_and_routed_writes_are_staged_and_a_refresh_reaches_the_engine` in crates/node/tests/writer_role.rs
- AC-9: `one_item_applied_alone_is_staged_too` in crates/node/tests/writer_role.rs
- AC-10: `a_write_waiting_on_a_shard_whose_stage_failed_is_refused_too` in crates/node/tests/writer_role.rs
