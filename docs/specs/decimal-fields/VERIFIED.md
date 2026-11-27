# Verified: `float` and `double` fields

- AC-1: `metrics_of_a_double_field_across_segments`
- AC-2: `metrics_of_a_float_field_widen_each_value_to_a_double`
- AC-3: `a_decimal_field_aggregates_the_query_matches_only`
- AC-4: `a_composite_over_a_decimal_field_is_refused_for_now`, `a_date_aggregation_of_a_double_fails_as_opensearch_fails_it`
- AC-5: `fields_the_node_cannot_read_yet_or_without_doc_values_fail_the_shard`
- AC-6: `a_term_query_on_a_double_matches_its_documents`, `a_terms_query_on_a_double_matches_any_value`, `queries_on_a_float_read_the_nearest_float`
- AC-7: `a_range_query_on_a_double_honours_each_bound`, `an_excluded_bound_steps_one_double_in`, `zero_steps_across_the_sign_as_lucenes_next_up_and_down_do`, `a_float_range_steps_one_float_in`
- AC-8: `a_value_that_is_not_a_finite_number_is_refused_as_the_mapper_refuses_it`
- AC-9: `a_double_sort_orders_across_segments_and_reads_back_doubles`, `a_float_sort_reads_back_floats`, `a_decimal_sort_puts_a_missing_value_last_in_either_direction`
- AC-10: `search_after_resumes_a_double_and_a_float_sort`
- AC-11: `docvalue_fields_write_a_double_and_a_float_widened_to_a_double`
- AC-12: `the_fields_api_reads_a_double_and_a_float_from_the_source`, `a_double_and_a_float_read_from_the_source_as_the_mapper_parses_them`
- AC-13: `a_histogram_of_a_double_keys_each_bucket_by_its_floor`, `a_histogram_of_a_float_reads_each_value_as_its_float`, `a_range_of_a_double_counts_the_documents_in_each_range`
- AC-14: `terms_of_a_double_key_each_bucket_by_its_value_and_write_the_double`, `terms_of_a_float_key_by_the_widened_double`
- AC-15: `a_terms_over_a_double_field_is_refused_rather_than_sent_as_long_terms`
