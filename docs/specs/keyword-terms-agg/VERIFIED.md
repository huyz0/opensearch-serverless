# Reader: `terms` aggregation over a keyword field: verified

Run on 2026-11-19 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `terms_count_across_segments_ordered_by_count_then_term_bytes`, `the_shard_keeps_shard_size_buckets_and_counts_the_rest_as_other`, `key_and_ascending_count_orders_list_as_opensearch_lists_them`, `the_buckets_are_those_of_the_querys_matches`
- AC-2: `min_doc_count_is_the_reduces_not_the_shards`, `an_unmapped_field_or_no_match_has_no_buckets`
- AC-3: `equal_keys_add_and_the_buckets_are_cut_at_size`, `count_ties_list_by_term_bytes_ascending`, `the_other_orders_list_as_opensearch_lists_them`, `min_doc_count_drops_buckets_without_counting_them_as_other`, `the_error_bound_is_the_numeric_terms`, `a_keyword_and_a_numeric_terms_do_not_reduce`, `a_keyword_terms_reduces_over_the_shards_that_answered` (crates/coordinator/tests/keyword_terms_reduce.rs)
- AC-4: `a_result_is_written_as_opensearch_writes_it`, `keys_are_escaped_as_json_strings`, `a_single_shard_result_is_written_reduced`
- AC-5: `a_keyword_terms_reserves_its_ordinals_and_keys_with_the_hits`
- AC-6: `a_cold_search_with_a_keyword_terms_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs): 3 requests, 3 round trips cold, 0 warm
- AC-7: `include_and_exclude_regexes_keep_the_accepted_terms`, `include_and_exclude_sets_keep_the_listed_terms`, `a_rejected_terms_documents_are_not_counted_as_other`, `a_zero_min_doc_count_lists_the_terms_no_match_holds`, `a_zero_min_doc_count_keeps_shard_size_buckets`, `a_missing_term_not_in_the_dictionary_is_a_bucket_among_the_ordinals`, `a_bad_pattern_fails_the_shard_as_opensearch_fails_it`, `keyword_only_options_are_refused_on_a_numeric_field`, `a_pattern_too_complex_to_determinize_is_refused`, `the_pattern_length_limit_counts_utf16_units`, `two_aggregations_of_a_field_have_their_own_filters`
- AC-8: `filters_and_empty_buckets_reserve_their_working_arrays`
- AC-9: `no_keyword_option_leaves_the_defaults`, `include_and_exclude_are_a_regex_or_a_sorted_set`, `missing_is_a_term`, `a_regex_and_a_set_do_not_mix`, `bad_filters_fail_as_opensearch_fails_them`, `partitions_and_non_string_missing_are_refused_as_not_supported_yet`
- AC-10: `metric_sub_aggregations_are_those_of_each_terms_documents`, `sub_aggregations_follow_the_querys_matches_and_empty_buckets_are_empty_metrics`, `a_sub_aggregation_of_an_unmapped_field_is_empty_in_every_bucket` (aggs/bucket/keyword_subs_tests.rs)
- AC-11: `equal_keys_reduce_their_sub_aggregations_and_the_cut_buckets_take_theirs`, `sub_aggregations_reduce_with_their_buckets_across_shards` (crates/coordinator/tests/keyword_terms_reduce.rs)
- AC-12: `a_bucket_writes_its_sub_aggregations_after_doc_count`
- AC-13: `sub_aggregations_reserve_their_states_with_the_terms`, `sub_aggregations_that_are_not_served_are_refused`
- AC-14: `an_inner_terms_counts_within_each_outer_bucket`, `an_inner_terms_of_an_unmapped_field_is_empty_and_a_numeric_one_is_refused`, `terms_nest_three_deep_and_deeper_is_refused`, `a_nested_keyword_terms_adds_no_request_to_a_cold_search` (crates/node/tests/it/aggs_budgets.rs)
- AC-15: `nested_results_reduce_and_write_inside_their_buckets` (aggs/bucket/keyword_terms_tests.rs)
- AC-16: `an_inner_terms_reserves_its_pairs_and_buckets`
- AC-17: `a_terms_orders_by_a_metric_sub_aggregation`, `nan_metrics_sort_last_and_ties_list_by_term`, `an_inner_terms_orders_within_each_outer_bucket` (aggs/bucket/keyword_order_tests.rs), `a_reduce_orders_by_the_reduced_sub_aggregation` (aggs/bucket/keyword_terms_tests.rs), `the_value_of_a_metric_for_sorting`, `nan_sorts_last_in_both_directions_and_ties_list_by_term` (aggs/bucket/sub_order_tests.rs)
- AC-18: `bad_order_paths_fail_as_opensearch_fails_them`, `order_paths_parse_as_aggregation_path_parses_them`, `bad_order_paths_are_refused_with_opensearchs_messages` (aggs/bucket/sub_order_tests.rs), `an_order_by_a_path_is_a_sub_aggregations` (aggs/bucket/terms_tests.rs)
- AC-19: `an_order_by_a_sub_aggregation_reserves_every_term`, `an_order_by_a_sub_aggregation_stops_at_the_bucket_limit` (aggs/bucket/keyword_order_tests.rs)
