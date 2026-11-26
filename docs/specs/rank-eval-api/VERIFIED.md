# Verified: `_rank_eval`

- AC-1: `precision_scores_the_relevant_share_of_the_hits`, `recall_and_reciprocal_rank_score_against_the_ratings` and `the_score_is_the_mean_of_the_requests` in crates/node/tests/it/rank_eval.rs, and the metric tests in crates/reader/src/rank_eval/metric.rs
- AC-2: `a_template_request_renders_then_ranks` in crates/node/tests/it/rank_eval.rs
- AC-3: `a_two_shard_index_ranks_through_the_coordinator` in crates/node/tests/it/rank_eval.rs
- AC-4: `a_search_that_fails_is_reported_under_failures` in crates/node/tests/it/rank_eval.rs
- AC-5: `a_request_that_is_not_a_valid_evaluation_is_a_400` in crates/node/tests/it/rank_eval.rs, and the parser tests in crates/reader/src/rank_eval/tests.rs
- AC-6: `what_this_node_lacks_is_refused` in crates/node/tests/it/rank_eval.rs
- AC-7: `details_come_in_the_order_java_writes_them` in crates/node/tests/it/rank_eval.rs, and `the_orders_match_the_java_maps` in crates/reader/src/rank_eval/order.rs
- AC-8: `a_warm_index_adds_no_store_request` in crates/node/tests/it/rank_eval.rs
- AC-9: `a_template_naming_a_stored_script_ranks_like_its_inline_twin` and `a_stored_template_without_a_source_set_is_refused` in crates/node/tests/it/rank_eval.rs; `a_template_may_name_a_stored_script` in crates/reader/src/rank_eval/tests.rs
