# Reader: open prefetch: verified

Run on 2026-11-19 (Linux cloud container, emulator store behind `Rounds` with a 20 ms call delay, provisional numbers).

- AC-1: `prefetch::tests::a_cold_open_reads_its_files_in_one_round_after_the_two_registers`, and the node's `a_cold_search_then_a_warm_one`. Seen red first: with the prefetch off the open made 10 calls at depth 7; on, the open is depth 3, and the cold search measures 3 requests in 3 round trips (was 6 and 6).
- AC-2: `prefetch::tests::a_shard_of_several_segments_costs_no_more_requests_and_the_same_depth`: 4 segments, 15 requests against 23 off (read twice by the engine), depth 3.
- AC-3: `neighbours_of_one_blob_share_a_range_and_blobs_never_do`, `a_gap_wider_than_the_limit_splits_a_range`, `a_large_unpinned_file_is_left_to_the_engine_and_a_pinned_one_is_not`, `the_plan_stops_at_the_byte_limit`.
- AC-4: `prefetch::tests::a_range_the_store_refuses_is_left_to_the_engine`.
- AC-5: `prefetch::tests::what_was_fetched_is_released_when_the_open_returns`.
- AC-6: `docs/request-budgets.md` rows `search.cold.*` and `search.metric_aggs.cold.*` at 3, enforced by `crates/node/tests/it/budgets.rs` and `aggs_budgets.rs`; the compat replay and reader tests pass.
