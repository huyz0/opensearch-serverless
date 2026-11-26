# Nested and reverse_nested aggregations: verified

Run on 2026-11-26 (Linux cloud container, emulator only). The parse tests were written first and did not compile until the types existed; the collect tests and the node test were written after the wiring and were not seen red.

- AC-1: `a_nested_reads_its_path_and_its_sub_aggregations`, `a_nested_without_a_path_is_refused_by_name`, `an_unknown_key_is_refused_with_its_token_and_name` (crates/reader/src/aggs/bucket/nested_agg_tests.rs)
- AC-2: `a_reverse_nested_has_an_optional_path`, `a_reverse_nested_holds_sub_aggregations`, `an_unknown_key_is_refused_with_its_token_and_name`
- AC-3: `a_nested_counts_the_children_of_the_documents_collected`, `a_nested_runs_its_sub_aggregations_over_the_children` (crates/reader/src/aggs/bucket/nested_agg_collect_tests.rs; sum under a nested is covered through the node test, value_count, avg and max are not run)
- AC-4: `a_reverse_nested_counts_each_parent_once_and_reads_its_fields`, `a_reverse_nested_with_a_path_hands_the_nested_documents_of_that_path`
- AC-5: `an_unmapped_path_or_a_leaf_field_counts_nothing_and_its_sub_aggregations_are_empty`, `a_reverse_nested_of_an_unmapped_path_counts_nothing_and_a_stray_one_fails`, and the object-not-nested refusal inside `nested_and_reverse_nested_aggregate_through_a_node`. The refusals carry the Java's message after the node's "failed to create query: " prefix, and answer 400 where the Java answers 500 (the plan step reports them as a bad request).
- AC-6: `the_results_of_shards_add`
- AC-7: `a_scored_sub_aggregation_is_refused_by_name_beneath_a_nested_at_any_depth`. The bucket count joins the bucket limit as a filter's does; no test.
- AC-8: `nested_and_reverse_nested_aggregate_through_a_node` (crates/node/tests/nested_aggs.rs)

NOT-RUN: a nested inside a nested (the testkit has no such index; the enclosing path is passed to the parents filter and not run); the engine's reservation of its parent and child lists; the sub-aggregations avg, max and value_count;  comparison with recorded OpenSearch answers; a nested under a keyword terms.
