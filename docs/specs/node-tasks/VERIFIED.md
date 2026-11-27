# Tasks for by-query and reindex: verified

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Coverage and mutants were not run in the sandbox.

- AC-1: `a_background_request_answers_a_task_that_finishes_with_the_synchronous_counts` and `update_by_query_and_reindex_run_as_tasks_too` in crates/node/tests/server/tasks_route.rs
- AC-2: `a_background_request_answers_a_task_that_finishes_with_the_synchronous_counts` in crates/node/tests/server/tasks_route.rs and `the_listing_has_running_tasks_only_and_filters_by_action` in crates/node/src/by_query/tasks/tests.rs
- AC-3: `cancel_stops_a_running_task_and_a_finished_one_is_not_found` in crates/node/tests/server/tasks_route.rs
- AC-4: `a_failing_task_has_an_error_and_a_refusal_stays_synchronous` in crates/node/tests/server/tasks_route.rs
- AC-5: `the_thirty_third_running_task_is_refused` in crates/node/tests/server/tasks_route.rs, `the_running_and_kept_bounds_hold`, `a_finished_task_expires_and_the_budget_bounds_a_start` and `an_entry_over_its_bound_drops_failures_and_truncates_text` in crates/node/src/by_query/tasks/tests.rs
- AC-6: `a_thread_that_ends_without_a_result_frees_its_slot` and `ids_start_at_the_start_time_and_other_nodes_are_refused` in crates/node/src/by_query/tasks/tests.rs
- AC-7: `another_nodes_or_a_malformed_id_is_refused` in crates/node/tests/server/tasks_route.rs and the request count in `a_background_request_answers_a_task_that_finishes_with_the_synchronous_counts`
- AC-8: `the_compatibility_notes_record_each_difference` in crates/node/src/by_query/tasks/tests.rs
