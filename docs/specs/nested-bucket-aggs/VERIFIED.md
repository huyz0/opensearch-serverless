# Reader: a bucket aggregation under a bucket: verified

Run on 2026-11-21 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java. Steps 1 to 3 serve a `range`, `histogram`, `date_histogram` or numeric `terms` under a `range`, `filters`, `filter`, `histogram`, `date_histogram` or numeric `terms`.

- AC-1: `a_range_under_a_range_counts_each_buckets_documents`, `a_histogram_under_a_range_has_its_own_buckets_in_each`, `a_date_histogram_under_a_range_has_its_own_buckets_in_each`, `shards_nested_ranges_add_bucket_for_bucket`, `a_range_under_a_histogram_counts_each_buckets_documents`, `a_histogram_under_a_date_histogram_has_its_own_buckets_in_each`, `a_nested_bucket_aggregation_under_a_histogram_reduces_across_shards`, `a_numeric_terms_under_a_range_counts_each_buckets_documents`, `a_range_under_a_numeric_terms_counts_each_buckets_documents`, `a_numeric_terms_under_a_numeric_terms_pairs_each_parent_with_its_values`, `a_nested_numeric_terms_reduces_across_shards`, `a_nested_numeric_terms_is_written_under_its_bucket`
- AC-2: `a_reduce_total_reports_a_fill_past_the_limit_not_a_mismatch`
- AC-3: `a_search_reserves_for_the_buckets_of_each_nested_level`, `a_histogram_parent_reserves_for_its_nested_levels_too`, `a_search_reserves_for_a_nested_numeric_terms`
- AC-4: `a_bucket_aggregation_under_a_keyword_terms_is_still_refused`, `a_numeric_terms_under_a_keyword_terms_is_refused` (terms_subs_tests)
- AC-5: `a_nested_numeric_terms_past_the_bucket_limit_fails_instead_of_running`
