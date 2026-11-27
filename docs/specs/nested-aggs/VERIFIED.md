# Nested and reverse_nested aggregations: verified

Run on 2026-11-26 (Linux cloud container, emulator only). The parse tests were written first and did not compile until the types existed; the collect tests and the node test were written after the wiring and were not seen red.

- AC-1: `a_nested_reads_its_path_and_its_sub_aggregations`, `a_nested_without_a_path_is_refused_by_name`, `an_unknown_key_is_refused_with_its_token_and_name` (crates/reader/src/aggs/bucket/nested_agg_tests.rs)
- AC-2: `a_reverse_nested_has_an_optional_path`, `a_reverse_nested_holds_sub_aggregations`, `an_unknown_key_is_refused_with_its_token_and_name`
- AC-3: `a_nested_counts_the_children_of_the_documents_collected`, `a_nested_runs_its_sub_aggregations_over_the_children` (crates/reader/src/aggs/bucket/nested_agg_collect_tests.rs; sum under a nested is covered through the node test, value_count, avg and max are not run)
- AC-4: `a_reverse_nested_counts_each_parent_once_and_reads_its_fields`, `a_reverse_nested_with_a_path_hands_the_nested_documents_of_that_path`
- AC-5: `an_unmapped_path_or_a_leaf_field_counts_nothing_and_its_sub_aggregations_are_empty`, `a_reverse_nested_of_an_unmapped_path_counts_nothing_and_a_stray_one_fails`, and the object-not-nested refusal inside `nested_and_reverse_nested_aggregate_through_a_node`., with the Java's message and a 500.
- AC-6: `the_results_of_shards_add`
- AC-7: `a_scored_sub_aggregation_is_refused_by_name_beneath_a_nested_at_any_depth`. The bucket count joins the bucket limit as a filter's does; no test.
- AC-8: `nested_and_reverse_nested_aggregate_through_a_node` (crates/node/tests/nested_aggs.rs)

- AC-3, AC-4 (nested in nested): `a_nested_inside_a_nested_joins_over_the_enclosing_objects_and_reverse_nested_steps_out`, `a_nested_on_the_grandchildren_alone_counts_them_under_their_roots` (over `Spec::with_nested_deep()`; the first was seen red, as a refusal, with the enclosing path dropped)

- AC-13: `avg_max_min_and_value_count_run_over_the_children`, `nested_numeric_sub_aggregations_reduce_across_shards` (crates/reader/src/aggs/bucket/nested_agg_collect_tests.rs; the testkit's children gained a `c.k` long; seen red with the field absent, green with it).
- AC-14: `a_pool_that_holds_the_plain_search_refuses_the_one_with_a_nested_aggregation` (crates/reader/src/search_nested_bytes_tests.rs, written after the code, not seen red); `nested_soak_ten_thousand_documents_answer_exactly` (crates/reader/src/aggs/bucket/nested_soak_tests.rs, `--ignored`): 10,000 documents, 20,000 children, four segments, debug build, emulator, 2 cores, provisional: nested query, nested agg with max, value_count and reverse_nested in one search, p50 2.9 ms, max 3.5 ms over five runs, answers exact.

NOT-RUN: the engine's real allocation for its parent and child lists (the figure is the reservation, not a measurement); a recorded stock answer for `min` under a nested (sum, max, avg and value_count are in `fixtures/compat/nested-search/nested-aggs.json`; recording `min` needs a stock node, which the sandbox declined to start).

- AC-9: `a_nested_under_a_keyword_terms_counts_each_buckets_own_children`, `a_nested_under_a_numeric_terms_counts_each_buckets_own_children`, `the_results_of_shards_add_under_a_terms` (crates/reader/src/aggs/bucket/nested_under_terms_tests.rs; written after the parse change and seen red as a parse refusal before it)
- AC-10: `a_reverse_nested_under_a_terms_counts_the_parents_of_each_term`, `a_reverse_nested_under_a_terms_outside_a_nested_is_refused_as_the_java_refuses`; through a node, `nested_and_reverse_nested_aggregate_through_a_node`
- AC-11: `a_nested_bucket_under_a_keyword_terms_adds_to_the_terms_reservation` (seen red with the addition removed: 668 vs 668 bytes), `a_nested_under_a_keyword_terms_is_reserved_with_what_it_holds`

- AC-12: `an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs), over `fixtures/compat/nested-search/` recorded from the stock 3.10.0 build b9f09249 and re-recorded to check the rules (`COMPAT_REPLAY_FROM`). The first replay found, and this change fixed: a reverse_nested outside a nested answered 500 where stock answers 400 illegal_argument_exception (check_placement, with `a_reverse_nested_of_an_unmapped_path_counts_nothing_and_a_stray_one_fails` and `a_reverse_nested_under_a_terms_outside_a_nested_is_refused_as_the_java_refuses` now asserting the 400); a nested query's scores ignored the child field's norms (0.315 where stock scores 0.291; `a_nested_query_scores_the_fields_of_its_child_query`), because the fields whose norms a search loads were not collected from inside the join; and a nested query on an object that is not nested said failed to find nested object where stock says nested object under path [p] is not of nested type.

NOT-RUN under a terms: a `nested` under a terms inside a `nested` more than one level deep; a `terms` ordered by a `nested` sub-aggregation's value.
