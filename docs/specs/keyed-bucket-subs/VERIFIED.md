# Reader: sub-aggregations of `histogram` and `date_histogram`: verified

Run on 2026-11-20 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `each_histogram_bucket_holds_the_metrics_of_its_documents`, `each_date_histogram_bucket_holds_the_metrics_of_its_documents`
- AC-2: `a_keyword_terms_under_a_histogram_counts_that_buckets_documents`
- AC-3: `a_histogram_over_an_unmapped_field_has_no_bucket_and_the_empty_values`
- AC-4: `a_bucket_sub_aggregation_under_a_histogram_is_refused`, `unserved_keys_and_orders_are_refused_as_not_supported_yet`, `unserved_keys_zones_and_orders_are_refused_as_not_supported_yet`
- AC-5: `a_reduce_adds_each_keys_sub_aggregations_and_fills_the_gaps_with_the_empty_value`, `a_reduce_keeps_each_buckets_values_with_its_bucket_in_the_ordered_answer`, `a_histogram_reduces_each_keys_sub_aggregations_over_the_shards` (crates/coordinator/tests/it/histogram_reduce.rs)
- AC-6: `a_buckets_sub_aggregations_follow_its_count_in_the_response`
- AC-7: `a_histograms_sub_aggregations_reserve_for_the_fields_values`, `the_search_reserves_a_histograms_sub_aggregations_with_its_hits`
- AC-8: `a_cold_search_with_metric_aggregations_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs, whose histogram, range and filters aggregations now carry a sub-aggregation)
