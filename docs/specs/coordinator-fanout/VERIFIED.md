# Verified: the coordinator asks its shards concurrently and reports who answered

- AC-1: `outcomes_come_in_item_order_whichever_finished_first` in crates/coordinator/tests/fanout.rs
- AC-2: `no_more_than_the_concurrency_are_in_flight` in crates/coordinator/tests/fanout.rs
- AC-2 (a wide window): `a_wide_window_uses_more_than_one_thread` in crates/coordinator/tests/fanout.rs
- AC-3: `a_failed_call_fails_only_its_own_slot` in crates/coordinator/tests/fanout.rs
- AC-4: `a_panicking_call_fails_only_its_own_slot` in crates/coordinator/tests/fanout.rs
- AC-5: `gather_splits_answers_failures_and_counts_coverage` in crates/coordinator/tests/fanout.rs
- AC-6: `nobody_answering_gives_the_lowest_failure_and_nothing_gives_nothing` in crates/coordinator/tests/fanout.rs
