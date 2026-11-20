# Reader: sub-aggregations of `range`, `filters` and `filter`: verified

Run on 2026-11-20 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `each_filters_bucket_holds_the_metrics_of_its_documents`, `each_range_bucket_holds_the_metrics_of_its_documents`, `a_filter_holds_the_metrics_of_its_documents`
- AC-2: `a_keyword_terms_under_a_bucket_counts_that_buckets_documents`
- AC-3: `a_range_over_an_unmapped_field_gives_each_bucket_the_empty_value`
- AC-4: `a_sub_aggregation_of_a_numeric_terms_under_a_bucket_is_refused`, `a_bucket_sub_aggregation_is_refused_as_not_supported_yet`, `a_filter_refuses_a_bucket_sub_aggregation_as_not_supported_yet`, `unserved_options_are_refused_as_not_supported_yet`
- AC-5: `a_filters_reduces_each_buckets_sub_aggregations_over_the_shards`, `a_range_reduces_each_buckets_sub_aggregations_over_the_shards` (crates/coordinator/tests), `a_reduce_keeps_no_sub_aggregation_when_no_shard_has_one`
- AC-6: `a_buckets_sub_aggregations_follow_its_count`, `a_ranges_sub_aggregations_follow_its_count`, `a_filter_holds_the_metrics_of_its_documents`
- AC-7: `the_sub_aggregations_of_a_bucket_reserve_their_states`
- AC-8: `a_cold_search_with_metric_aggregations_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs, whose range and filters aggregations now carry a sub-aggregation)
