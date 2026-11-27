# Reader: `terms` aggregation over a numeric field: verified

Run on 2026-11-19 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `defaults_are_opensearchs`, `shard_size_follows_size_unless_asked_or_ordered_by_key`, `orders_parse`, `min_doc_count_parses`, `bad_options_fail_as_opensearch_fails_them`, `unserved_options_are_refused_as_not_supported_yet`, `sub_aggregations_are_refused_as_not_supported_yet`
- AC-2: `every_value_is_a_bucket_ordered_by_count_then_key_and_cut_at_size`, `key_orders_list_as_opensearch_lists_them`, `the_buckets_are_those_of_the_querys_matches`
- AC-3: `min_doc_count_is_the_reduces_not_the_shards`, `min_doc_count_drops_buckets_without_counting_them_as_other`, `an_unmapped_field_or_no_match_has_no_buckets`, `a_text_field_fails_the_shard`
- AC-4: `a_date_key_is_also_written_as_text`
- AC-5: `equal_keys_add_and_the_buckets_are_cut_at_size`, `a_full_shard_may_have_cut_what_its_last_bucket_counts`, `one_shard_has_no_error_even_when_full`, `an_ascending_count_is_unbounded_and_a_key_order_is_exact`, `key_descending_lists_keys_descending`, `count_ties_list_by_key_ascending`, `a_terms_reduces_over_the_shards_that_answered` (crates/coordinator/tests/it/terms_reduce.rs)
- AC-6: `a_result_is_written_as_opensearch_writes_it`, `a_single_shard_result_is_written_reduced`
- AC-7: `a_terms_reserves_its_buckets_and_counts_with_the_hits`, `more_distinct_values_than_the_bucket_limit_fail_the_shard`
- AC-8: `a_cold_search_with_metric_aggregations_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs)
