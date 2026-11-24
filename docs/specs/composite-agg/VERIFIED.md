# Reader: the `composite` aggregation: verified

Run on 2026-11-23 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java. The request counts are measured on the in-process emulator, so provisional.

- AC-1: `a_keyword_source_lists_terms_in_order_and_pages_with_after`, `a_result_is_written_as_opensearch_writes_it`, `the_buckets_are_those_of_the_querys_matches_and_a_search_writes_them`, `a_long_source_keys_by_the_value_and_resumes_after_a_number_or_its_text`
- AC-2: `several_sources_key_by_every_value`, `several_sources_order_by_the_first_then_the_next`
- AC-3: `paging_with_after_visits_every_bucket_once_in_the_unpaged_order`, `an_after_opensearch_refuses_is_refused`
- AC-4: `the_missing_key_sorts_by_missing_order_and_the_default_follows_the_order`, `missing_bucket_on_a_field_every_document_has_adds_no_bucket`, `an_unmapped_source_has_no_buckets_unless_it_asks_for_the_missing_one`
- AC-5: `a_histogram_source_needs_a_positive_interval`, `several_sources_key_by_every_value`
- AC-6: `equal_keys_add_and_the_buckets_are_cut_at_size`, `a_descending_source_reduces_in_descending_order`, `the_reduce_counts_its_buckets_against_the_limit_and_keeps_kinds_apart`, `a_size_over_the_bucket_limit_fails_as_too_many_buckets`
- AC-7: `the_body_is_parsed_into_sources_size_and_after`, `a_body_opensearch_refuses_is_refused`, `what_this_node_does_not_serve_is_refused_by_name`, `a_source_the_shard_cannot_read_fails_it`
- AC-8: `a_composite_reserves_its_keys_and_ordinals_with_the_hits`
- AC-9: `a_cold_search_with_a_composite_then_a_warm_one`
