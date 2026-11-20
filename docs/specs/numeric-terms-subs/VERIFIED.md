# Reader: sub-aggregations of a numeric `terms`: verified

Run on 2026-11-20 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `each_numeric_terms_bucket_holds_the_metrics_of_its_documents`
- AC-2: `a_keyword_terms_under_a_numeric_terms_counts_that_buckets_documents`
- AC-3: `a_numeric_terms_over_an_unmapped_field_has_no_bucket_and_the_empty_values`
- AC-4: `a_numeric_terms_under_a_numeric_terms_is_refused`
- AC-5: `a_reduce_adds_each_keys_sub_aggregations_and_keeps_them_with_the_kept_buckets`
- AC-6: `a_buckets_sub_aggregations_follow_its_count_in_the_response`
- AC-7: `a_numeric_terms_reserves_for_the_fields_values_and_a_keyword_one_here_nothing`
- AC-8: `a_cold_search_with_metric_aggregations_then_a_warm_one` (crates/node/tests/it/aggs_budgets.rs, whose numeric terms now carries a sub-aggregation)
