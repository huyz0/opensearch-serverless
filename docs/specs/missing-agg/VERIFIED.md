# Reader: `missing` aggregation: verified

Run on 2026-11-27 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java.

- AC-1: `a_missing_body_is_the_filter_of_documents_without_the_field`, `a_missing_body_without_a_field_or_with_a_bad_key_fails_as_range_does`
- AC-2: `documents_without_the_field_count_and_an_unmapped_field_counts_every_match`
- AC-3: the result is the one the single-filter aggregation reduces and writes, held by `a_single_result_writes_only_its_count` and `a_single_and_a_multiple_result_do_not_reduce_together` (filters_tests.rs); `a_sub_aggregation_runs_over_the_missing_documents_only` reads the single result
- AC-4: `a_sub_aggregation_runs_over_the_missing_documents_only`, `a_missing_runs_as_a_sub_aggregation_over_its_parents_documents`
