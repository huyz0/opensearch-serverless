# Reader: `terms` aggregation over a keyword field: verified

Run on 2026-11-19 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `terms_count_across_segments_ordered_by_count_then_term_bytes`, `the_shard_keeps_shard_size_buckets_and_counts_the_rest_as_other`, `key_and_ascending_count_orders_list_as_opensearch_lists_them`, `the_buckets_are_those_of_the_querys_matches`
- AC-2: `min_doc_count_is_the_reduces_not_the_shards`, `an_unmapped_field_or_no_match_has_no_buckets`, `a_zero_min_doc_count_is_refused_on_a_keyword_field`
- AC-3: `equal_keys_add_and_the_buckets_are_cut_at_size`, `count_ties_list_by_term_bytes_ascending`, `the_other_orders_list_as_opensearch_lists_them`, `min_doc_count_drops_buckets_without_counting_them_as_other`, `the_error_bound_is_the_numeric_terms`, `a_keyword_and_a_numeric_terms_do_not_reduce`, `a_keyword_terms_reduces_over_the_shards_that_answered` (crates/coordinator/tests/keyword_terms_reduce.rs)
- AC-4: `a_result_is_written_as_opensearch_writes_it`, `keys_are_escaped_as_json_strings`, `a_single_shard_result_is_written_reduced`
- AC-5: `a_keyword_terms_reserves_its_ordinals_and_keys_with_the_hits`
- AC-6: `a_cold_search_with_a_keyword_terms_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs): 3 requests, 3 round trips cold, 0 warm
