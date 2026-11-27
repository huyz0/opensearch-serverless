# Reader: `range` aggregation over a numeric field: verified

Run on 2026-11-19 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `ranges_parse_with_open_ends_and_sort_by_from_then_to`, `equal_ranges_keep_request_order_and_null_ends_are_open`, `bad_bodies_fail_as_opensearch_fails_them`, `unserved_options_are_refused_as_not_supported_yet`
- AC-2: `each_range_counts_the_documents_from_inclusive_to_exclusive`, `the_counts_are_those_of_the_querys_matches`
- AC-3: `an_unmapped_field_or_no_match_counts_nothing_in_every_range`, `a_keyword_or_text_field_fails_the_shard`
- AC-4: `a_date_range_also_writes_its_bounds_as_text`
- AC-5: `counts_add_per_range_across_shards`, `a_reduce_needs_parts_of_one_shape`, `a_range_adds_its_counts_over_the_shards_that_answered` (crates/coordinator/tests/it/range_reduce.rs)
- AC-6: `an_array_result_is_written_as_opensearch_writes_it`, `a_keyed_result_names_each_bucket_by_its_key`
- AC-7: `more_ranges_than_the_bucket_limit_fail_the_shard`, `a_range_reserves_its_buckets_with_the_hits`
- AC-8: `a_cold_search_with_metric_aggregations_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs)
