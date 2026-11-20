# Reader: `filter` aggregation: verified

Run on 2026-11-20 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `a_filter_body_is_one_query_under_the_key_zero`, `a_malformed_filter_fails_as_a_malformed_query_does`, `a_filter_refuses_a_bucket_sub_aggregation_as_not_supported_yet`
- AC-2: `a_filter_counts_the_documents_it_matches`
- AC-5: `a_single_and_a_multiple_result_do_not_reduce_together`
- AC-6: `a_single_result_writes_only_its_count`
