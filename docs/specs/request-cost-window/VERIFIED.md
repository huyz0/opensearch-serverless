# A write reports its share of its commit window: verified

Run on 2026-11-25 (Linux sandbox; emulator numbers are provisional).

- AC-1: `a_splitting_scope_gives_each_member_its_weights_share` in crates/blob/src/cost.rs. Seen failing before the splitting scope existed (it did not compile).
- AC-2: `a_splitting_scope_carries_to_another_thread_and_ends_with_its_guard` in crates/blob/src/cost.rs
- AC-3: `two_writers_sharing_a_window_each_report_half_its_put` in crates/writer/tests/it/node_log_cost_window.rs. Seen failing before the window kept its writers (one reported 0).
- AC-4: `a_writer_that_did_not_ask_leaves_its_share_unreported` in crates/writer/tests/it/node_log_cost_window.rs. Seen failing before the window kept its writers (the asking writer reported the whole PUT, 1000).
- AC-5: `a_writer_with_a_window_to_itself_reports_the_whole_put` and the two above in crates/writer/tests/it/node_log_cost_window.rs
- AC-6: `the_attempts_of_a_hedged_put_are_shared_out_like_a_single_one` in crates/writer/tests/it/node_log_cost_window.rs. Seen failing with the hedge thread's scope entry removed (0 of 1000).
- AC-7: `a_writer_that_did_not_ask_leaves_its_share_unreported` and `two_writers_sharing_a_window_each_report_half_its_put` in crates/writer/tests/it/node_log_cost_window.rs, which check after each commit that the writer's own scope is back and that one that did not ask has none.
- AC-8: `a_write_that_asks_is_told_its_share_of_the_log_put` in crates/node/tests/server/boot_usage.rs. A smoke of the whole path; it was not seen failing, because a lone writer reported the whole PUT before the change too.

NOT-RUN: coverage and mutants (left to CI); a measurement of the cost on the write path (a thread-local read and, for an asking writer, a small allocation; no store request changes).
