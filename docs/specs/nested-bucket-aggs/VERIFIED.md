# Reader: a bucket aggregation under a bucket: verified

Run on 2026-11-21 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java. Step 1 serves a `range`, `histogram` or `date_histogram` under a `range`, `filters` or `filter`; the criteria below name what it holds, and the rest wait for their steps.

- AC-1: `a_range_under_a_range_counts_each_buckets_documents`, `a_histogram_under_a_range_has_its_own_buckets_in_each`, `a_date_histogram_under_a_range_has_its_own_buckets_in_each`
- AC-3: `a_reduce_total_reports_a_fill_past_the_limit_not_a_mismatch`, `shards_nested_ranges_add_bucket_for_bucket`
- AC-6: `a_search_reserves_for_the_buckets_of_each_nested_level`
- AC-8: `a_bucket_aggregation_under_a_keyword_terms_is_still_refused`
