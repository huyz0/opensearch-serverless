# Multi terms aggregation: verified

Run on 2026-11-27 (Linux cloud container). The response bodies and the reduce come from `fixtures/multi-terms/java-multi-terms-3.10.0.txt`, which `MultiTermsRecordings.java` printed from the OpenSearch 3.10.0 jars; the parse messages come from the jar's constants. The shard's own buckets (what `MultiTermsAggregator` keeps) are read from the Java and not recorded: no stock 3.10 build runs here. The tests were written after the code and passed at their first run apart from two mistakes of the tests themselves, so none was seen red first.

- AC-1: `the_body_parses_with_its_defaults_and_options`, `bad_and_unserved_options_fail_as_java_fails` (`oss-reader`)
- AC-2: `a_reduce_is_written_as_the_bytes_java_wrote`
- AC-3: `equal_counts_order_by_key_term_by_term`
- AC-4: `a_shard_counts_the_combinations_and_keeps_the_best_by_the_order`, `an_unmapped_term_answers_nothing`
- AC-5: `a_shard_counts_the_combinations_and_keeps_the_best_by_the_order`, `a_shard_minimum_drops_the_buckets_under_it_into_the_other_documents`
- AC-6: `bad_and_unserved_options_fail_as_java_fails`, `sub_aggregations_and_nesting_are_refused`, `a_term_the_shard_cannot_read_is_refused`; the refusal of a date term and the `too_many_buckets_exception` past 65,535 combinations are NOT-RUN: no test index maps a date field or holds that many combinations
- AC-7: `the_error_bound_shows_as_java_leaves_it_on_the_first_part_holding_a_key`, `show_term_doc_count_error_parses_as_a_boolean` (`crates/reader/src/aggs/bucket/multi_tests.rs`); on the forwarded wire `a_multi_terms_answer_is_the_bytes_java_wrote`, `the_multi_terms_builders_are_the_bytes_java_wrote` (`crates/transport/tests/search_shard_rare_multi.rs`) and `a_multi_terms_that_shows_its_error_bound_writes_each_buckets` (`oss-node`)
