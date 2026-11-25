# Search pipelines: verified

Run on 2026-11-25 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-reader --test search_pipeline`, `cargo test -p oss-coordinator --test search_pipeline_admin`, `cargo test -p oss-node --test it -- search_pipeline frozen_search`, `cargo test -p oss-node --features coordinator --test it search_pipeline`, `cargo test -p oss-node --all-features --test server -- search_pipeline_route`, `cargo clippy --all-targets --all-features -D warnings` for the three crates.

The expectations come from reading the Java source (`SearchPlanner`, `SearchPipelineHandler`, the common module's processors), not from a recorded run of it.

- AC-1: `a_definition_of_supported_processors_is_accepted`, `a_bad_definition_is_invalid`, `a_supported_definition_is_accepted_and_a_wrong_one_is_400`
- AC-2: `what_opensearch_ships_and_this_node_does_not_build_is_unsupported_by_name`, `a_processor_this_node_does_not_build_is_501_by_name`, `what_the_node_cannot_run_is_refused_and_stores_nothing`
- AC-3: `a_mutation_names_one_concrete_id_and_only_put_stores`
- AC-4: `a_pipeline_is_stored_read_listed_replaced_and_removed`
- AC-5: `the_routes_and_the_parameter_exist_only_when_search_pipelines_are_on`, `without_a_pipeline_source_naming_one_is_501`
- AC-6: `oversample_asks_for_more_and_truncate_cuts_back`, `oversample_then_truncate_asks_for_more_and_cuts_back`, `the_factor_rounds_up`
- AC-7: `truncate_hits_cuts_to_its_target_and_fails_without_one`, `truncate_without_a_target_or_an_earlier_oversample_fails`, `truncate_hits_uses_its_own_target_and_the_smallest_wins`
- AC-8: `filter_query_narrows_a_search`, `filter_query_narrows_a_query_and_a_search_with_none`, `a_context_prefix_keeps_pairs_apart`
- AC-9: `a_sharded_index_cuts_the_merged_hits`, `a_search_of_a_view_takes_a_search_pipeline`
- AC-10: `a_missing_pipeline_is_400_and_none_runs_nothing`
- AC-11: `a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed`, `a_search_resolves_its_pipeline_with_one_marker_get_and_no_list`, `a_search_naming_no_pipeline_makes_no_request_for_the_set`
- AC-12: `refusals_come_in_the_shells_order_and_words`
- AC-13: NOT-RUN: no gate; measured once with a throwaway test that is not kept: 3,000 searches of size 3 match_all over the compat index in one process, three rounds after a 300 search warm-up. Without the parameter 358 to 382 microseconds a search; with search_pipeline=_none 331 to 375 (the same within noise); with an oversample and truncate_hits pair held in memory 407 to 441 (it asks the shard for twice the hits). Provisional: emulator, shared sandbox.
