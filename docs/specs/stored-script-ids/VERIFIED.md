# Stored Painless scripts named by id: verified

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Coverage and mutants were not run in the sandbox.

- AC-1: `script_score_by_id_scores_like_the_same_script_inline_and_resolves_once` in crates/node/tests/stored_script_ids.rs
- AC-2: `script_fields_sort_query_and_scripted_metric_run_a_stored_script` in crates/node/tests/stored_script_ids.rs
- AC-3: `update_by_query_and_reindex_run_a_stored_script` in crates/node/tests/server/stored_script_route.rs and `an_update_body_takes_a_stored_script_by_id` in crates/node/src/update_body.rs
- AC-4: `a_missing_mixed_or_foreign_stored_script_is_refused` in crates/node/tests/stored_script_ids.rs, `a_missing_mixed_or_foreign_id_and_a_node_with_no_resolver_are_refused` in crates/node/tests/server/stored_script_route.rs and `what_a_stored_id_may_not_be_is_refused` in crates/node/src/update_body.rs
- AC-5: `a_node_that_resolves_nothing_makes_no_request_and_a_ttl_makes_one_marker_get` and `a_changed_script_is_seen_after_the_ttl_and_a_missing_one_is_missing` in crates/node/src/stored_script_source/tests.rs
- AC-6: `a_set_past_the_bound_and_an_unreadable_script_are_refused` in crates/node/src/stored_script_source/tests.rs
- AC-7: `script_score_by_id_scores_like_the_same_script_inline_and_resolves_once` in crates/node/tests/stored_script_ids.rs and `resolves_that_arrive_together_make_one_refresh` in crates/node/src/stored_script_source/tests.rs
- AC-8: `a_put_here_is_seen_at_the_next_resolve_even_after_a_check_that_was_in_flight` in crates/node/src/stored_script_source/tests.rs and `the_node_that_took_a_put_or_delete_sees_it_at_once` in crates/node/tests/server/stored_script_route.rs
- AC-9: `the_stored_script_ttl_defaults_to_ten_seconds_and_is_bounded` in crates/node/src/boot/config/tests/resident.rs and the Stored scripts by id paragraph of docs/compatibility.md

Not run: a recording of a stock node for the 404 on a missing id and the refusal of `lang` beside `id`; both follow OpenSearch's documented behaviour.
