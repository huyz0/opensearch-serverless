# Writes to one shard share a commit window: evidence

- AC-1: `records_staged_one_after_another_share_a_window_and_a_put`, `staging_no_records_stages_nothing`, `finishing_a_staged_window_writes_it_without_a_wait`, `finishing_a_failed_window_reports_the_failure_to_the_wait` (crates/writer/tests/node_log_staged.rs).
- AC-2: `at_depth_one_the_groups_of_a_shard_go_one_at_a_time` (crates/node/tests/writer_pipeline.rs).
- AC-3: `groups_in_flight_together_all_land_in_one_put` (crates/node/tests/writer_pipeline.rs).
- AC-4: `a_failed_put_fails_every_group_in_flight_and_the_shard_is_released_by_the_last`, `a_group_staged_after_a_failure_is_refused_not_hung` (crates/node/tests/writer_pipeline.rs).
- AC-5: `a_retire_while_groups_are_in_flight_loses_none_of_the_acknowledged` (crates/node/tests/writer_pipeline.rs).
- AC-6: `a_panic_while_acknowledging_fails_the_slot_and_hangs_nothing` (crates/node/tests/writer_pipeline.rs).
- AC-7: `writes_to_one_index_share_a_window_with_a_pipeline_and_not_without_one` (crates/node/tests/server/boot_write_concurrency.rs).
- AC-8: `sixty_four_small_bulks_in_flight_together_share_a_window` (crates/node/tests/server/boot_write_concurrency.rs). Seen failing with the old budget share: 64 bulks took 2.03 s.

Scaling (provisional, crates/node/tests/store_usage_measure.rs with `MEASURE_DEPTH`, a debug build on 4 cores over a file store, bulks of 20 documents, a 250 ms window, 64 bulk holders): clients on one index, p50 a bulk, depth 1 against depth 16: 16 clients 3,803 ms against 254 ms; 64 clients 10,388 ms against 521 ms. Depth 1 with 16 and 64 clients also saw the write limiter refuse 52 and 198 bulks as the object store looked slow (an append taking about 5 s); depth 16 refused none at 16 clients and 69 at 64.
