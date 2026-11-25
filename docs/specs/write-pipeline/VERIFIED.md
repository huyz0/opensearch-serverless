# Writes to one shard share a commit window: evidence

- AC-1: `records_staged_one_after_another_share_a_window_and_a_put`, `staging_no_records_stages_nothing`, `finishing_a_staged_window_writes_it_without_a_wait`, `finishing_a_failed_window_reports_the_failure_to_the_wait` (crates/writer/tests/node_log_staged.rs).
- AC-2: `at_depth_one_the_groups_of_a_shard_go_one_at_a_time` (crates/node/tests/writer_pipeline.rs).
- AC-3: `groups_in_flight_together_all_land_in_one_put` (crates/node/tests/writer_pipeline.rs).
- AC-4: `a_failed_put_fails_every_group_in_flight_and_the_shard_is_released_by_the_last`, `a_group_staged_after_a_failure_is_refused_not_hung` (crates/node/tests/writer_pipeline.rs).
- AC-5: `a_retire_while_groups_are_in_flight_loses_none_of_the_acknowledged` (crates/node/tests/writer_pipeline.rs).
- AC-6: `a_panic_while_acknowledging_fails_the_slot_and_hangs_nothing` (crates/node/tests/writer_pipeline.rs).
- AC-7: `writes_to_one_index_share_a_window_with_a_pipeline_and_not_without_one` (crates/node/tests/server/boot_write_concurrency.rs).
- AC-8: `sixty_four_small_bulks_in_flight_together_share_a_window` (crates/node/tests/server/boot_write_concurrency.rs). Seen failing with the old budget share: 64 bulks took 2.03 s.
- AC-9: `a_refresh_while_groups_are_in_flight_waits_for_them_and_returns` (crates/node/tests/writer_pipeline.rs).
- AC-10: `a_refresh_after_a_failed_window_does_not_hang_and_the_shard_goes_on` (crates/node/tests/writer_pipeline.rs).
- AC-11: `a_depth_of_two_bounds_the_groups_of_a_shard_in_flight` (crates/node/tests/writer_pipeline.rs).
- AC-12: `a_node_that_takes_over_after_a_pipelined_node_keeps_every_acknowledged_write` (crates/node/tests/writer_pipeline.rs).
- AC-13: `groups_are_acknowledged_to_the_tail_in_sequence_order` (crates/node/tests/writer_pipeline.rs).
- AC-14: `at_the_largest_depth_sixty_four_groups_land_each_with_its_own_number` (crates/node/tests/writer_pipeline.rs).
- AC-15: `a_bulk_with_one_large_item_among_small_ones_is_applied_with_a_pipeline` (crates/node/tests/server/boot_write_concurrency.rs).
- AC-16: `a_field_a_pipelined_write_introduces_is_held_in_the_descriptor` (crates/node/tests/server/boot_write_concurrency.rs). Seen failing before the pipelined path held the growth (the mapping read back without the field).

Scaling (provisional, crates/node/tests/store_usage_measure.rs with `MEASURE_DEPTH`, a debug build on 4 cores over a file store, bulks of 20 documents, a 250 ms window, 64 bulk holders): clients on one index, p50 a bulk, depth 1 against depth 16: 16 clients 3,803 ms against 254 ms; 64 clients 10,388 ms against 521 ms. Depth 1 with 16 and 64 clients also saw the write limiter refuse 52 and 198 bulks as the object store looked slow (an append taking about 5 s); depth 16 refused none at 16 clients and 69 at 64.

The default stays 1 until the rest of the ADR's tests exist. At depth 8: the whole `oss-node` test suite passes with depth 8; at one client p50 254.8 ms and p99 259.1 ms at depth 1 against 255.1 and 259.6 ms at depth 16 (provisional, same setup), a difference under 1%; PUTs a window are unchanged (AC-3 asserts no more PUTs than groups).
