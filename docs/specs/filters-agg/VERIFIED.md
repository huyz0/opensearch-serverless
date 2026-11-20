# Reader: `filters` aggregation: verified

Run on 2026-11-20 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `an_array_of_filters_is_keyed_by_position_in_request_order`, `an_object_of_filters_is_keyed_and_sorted_by_key`, `the_other_bucket_is_on_by_its_flag_or_its_key`, `bad_bodies_fail_as_opensearch_fails_them`, `sub_aggregations_are_refused_as_not_supported_yet`
- AC-2: `each_filter_counts_the_documents_it_matches_and_other_the_rest`, `a_document_counts_in_every_filter_it_matches_and_other_is_what_none_match`, `the_counts_are_those_of_the_querys_matches`
- AC-3: `a_query_matching_nothing_counts_zero_and_a_filter_on_an_unmapped_field_matches_none`, `a_filter_the_shard_cannot_build_fails_the_search_as_a_query_does`
- AC-5: `counts_add_per_bucket_across_shards`, `a_reduce_needs_parts_of_one_shape`, `a_filters_adds_its_counts_over_the_shards_that_answered` (crates/coordinator/tests/filters_reduce.rs)
- AC-6: `an_array_result_lists_counts_in_order`, `a_keyed_result_names_each_bucket_by_its_key`
- AC-7: `more_buckets_than_the_bucket_limit_fail_the_shard`, `a_filters_reserves_its_buckets_with_the_hits`
- AC-8: `a_cold_search_with_metric_aggregations_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs)
